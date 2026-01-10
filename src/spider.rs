use crate::{storage, AppState};
use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::time::{Duration, interval};

// Minimal BEP-5 DHT “spider”:
// - Joins the DHT via bootstrap nodes (find_node)
// - Responds to incoming queries so other nodes keep us in their routing tables
// - Harvests info_hash from announce_peer / get_peers queries
// - Learns more nodes from responses (“nodes” compact format)

// Default to an ephemeral UDP port so this works smoothly on home networks
// (no need to open/forward a port).
const DEFAULT_BIND: &str = "0.0.0.0:0";
const DEFAULT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
];

const MAX_KNOWN_NODES: usize = 10_000;

// Dedupe for incoming/sampled info-hashes.
//
// Goal: avoid hammering sled with repeated upserts for the same hot hash.
// We use a rotating Bloom filter (two windows) to keep memory bounded and
// prevent "clear-and-thrash" behavior under high cardinality.
const SEEN_ROTATE_EVERY: Duration = Duration::from_secs(15 * 60);
const SEEN_BITS_POW2: u32 = 26; // 2^26 bits ~= 8 MiB per filter, 16 MiB total (two windows)
const SEEN_K: u8 = 12;

const SAMPLE_EVERY: Duration = Duration::from_secs(5);
const SAMPLE_PER_TICK: usize = 12;
const MAX_SAMPLES_PER_MSG: usize = 256;

pub async fn run(state: AppState) {
    // Allow disabling the spider entirely.
    if std::env::var("SERMA_SPIDER")
        .ok()
        .is_some_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"))
    {
        tracing::info!("spider: disabled via SERMA_SPIDER");
        return;
    }

    let bind = std::env::var("SERMA_SPIDER_BIND")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BIND.to_string());

    // Use separate IPv4 + (optional) IPv6 UDP sockets so we can talk to both
    // families regardless of OS IPv6 dual-stack settings.
    let primary = match UdpSocket::bind(&bind).await {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(%err, bind = %bind, "spider: failed to bind; trying ephemeral v4");
            match UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(%err, "spider: failed to bind UDP socket; spider disabled");
                    return;
                }
            }
        }
    };

    let primary_addr = match primary.local_addr() {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!(%err, "spider: failed to get local addr");
            return;
        }
    };

    let (socket_v4, socket_v6): (Option<UdpSocket>, Option<UdpSocket>) = if primary_addr.is_ipv4()
    {
        let socket_v6 = match UdpSocket::bind("[::]:0").await {
            Ok(s) => Some(s),
            Err(err) => {
                tracing::debug!(%err, "spider: ipv6 udp bind failed; continuing with ipv4 only");
                None
            }
        };
        (Some(primary), socket_v6)
    } else {
        let socket_v4 = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => Some(s),
            Err(err) => {
                tracing::debug!(%err, "spider: ipv4 udp bind failed; continuing with ipv6 only");
                None
            }
        };
        (socket_v4, Some(primary))
    };

    // DHT node id: 20 random-ish bytes. We reuse rbit’s peer-id generator (also 20 bytes).
    let node_id = *rbit::peer::PeerId::generate().as_bytes();

    if let Some(sock) = socket_v4.as_ref() {
        if let Ok(a) = sock.local_addr() {
            tracing::info!(bind=%a, "spider: listening (ipv4)");
        }
    }
    if let Some(sock) = socket_v6.as_ref() {
        if let Ok(a) = sock.local_addr() {
            tracing::info!(bind=%a, "spider: listening (ipv6)");
        }
    }

    let mut known_nodes: VecDeque<SocketAddr> = VecDeque::new();
    let mut known_set: HashSet<SocketAddr> = HashSet::new();

    let mut seen_hashes = RollingBloom::new(SEEN_BITS_POW2, SEEN_K, SEEN_ROTATE_EVERY);

    // Bootstrap right away.
    for addr in resolve_bootstrap().await {
        push_node(addr, &mut known_nodes, &mut known_set);
    }
    bootstrap_tick(socket_v4.as_ref(), socket_v6.as_ref(), &node_id, &mut known_nodes).await;

    // Actively sample info-hashes from the network (BEP-51) so we still discover
    // content even when we're behind NAT and not receiving unsolicited queries.
    sample_tick(socket_v4.as_ref(), socket_v6.as_ref(), &node_id, &mut known_nodes).await;

    let mut boot_int = interval(Duration::from_secs(15));
    let mut gc_int = interval(Duration::from_secs(30));
    let mut sample_int = interval(SAMPLE_EVERY);

    let mut buf4 = vec![0u8; 4096];
    let mut buf6 = vec![0u8; 4096];
    loop {
        tokio::select! {
            _ = boot_int.tick() => {
                bootstrap_tick(socket_v4.as_ref(), socket_v6.as_ref(), &node_id, &mut known_nodes).await;
            }
            _ = sample_int.tick() => {
                sample_tick(socket_v4.as_ref(), socket_v6.as_ref(), &node_id, &mut known_nodes).await;
            }
            _ = gc_int.tick() => {
                // Keep the rolling Bloom filter fresh.
                seen_hashes.maybe_rotate();
                if known_nodes.len() > MAX_KNOWN_NODES {
                    while known_nodes.len() > MAX_KNOWN_NODES {
                        if let Some(old) = known_nodes.pop_front() {
                            known_set.remove(&old);
                        } else {
                            break;
                        }
                    }
                }
            }
            recv = recv_from_any(socket_v4.as_ref(), socket_v6.as_ref(), &mut buf4, &mut buf6) => {
                let Some((n, from, fam)) = recv else {
                    continue;
                };
                if n == 0 {
                    continue;
                }

                let raw = if fam == 4 { &buf4[..n] } else { &buf6[..n] };
                if let Some(msg) = KrpcMessage::decode(raw) {
                    // Learn nodes from responses.
                    if let Some(nodes) = msg.compact_nodes() {
                        for addr in parse_compact_nodes(nodes) {
                            push_node(addr, &mut known_nodes, &mut known_set);
                        }
                    }
                    if let Some(nodes6) = msg.compact_nodes_v6() {
                        for addr in parse_compact_nodes_v6(nodes6) {
                            push_node(addr, &mut known_nodes, &mut known_set);
                        }
                    }

                    // Active discovery: harvest info_hash from BEP-51 sample_infohashes responses.
                    if let Some(samples) = msg.samples_from_response() {
                        for chunk in samples.chunks_exact(20).take(MAX_SAMPLES_PER_MSG) {
                            let mut info_hash = [0u8; 20];
                            info_hash.copy_from_slice(chunk);
                            if should_accept_hash(&mut seen_hashes, info_hash) {
                                let info_hex = hex::encode(info_hash);
                                if let Err(err) = ingest_spidered_hash(&state, &info_hex) {
                                    tracing::debug!(%err, hash=%info_hex, "spider: ingest failed");
                                } else {
                                    tracing::info!(hash=%info_hex, "spider: sampled");
                                }
                            }
                        }
                    }

                    // Harvest info_hash from incoming queries.
                    if let Some(info_hash) = msg.info_hash_from_query() {
                        if should_accept_hash(&mut seen_hashes, info_hash) {
                            let info_hex = hex::encode(info_hash);

                            // Store + index.
                            if let Err(err) = ingest_spidered_hash(&state, &info_hex) {
                                tracing::debug!(%err, hash=%info_hex, "spider: ingest failed");
                            } else {
                                tracing::info!(hash=%info_hex, "spider: discovered");
                            }
                        }
                    }

                    // Respond to queries so we remain a "good" node.
                    if msg.is_query() {
                        if let Some(resp) = msg.make_minimal_response(&node_id) {
                            send_to_family(socket_v4.as_ref(), socket_v6.as_ref(), &resp, from)
                                .await;
                        }
                    }

                    // If we get a query from this node, keep it as known.
                    if msg.is_query() {
                        push_node(from, &mut known_nodes, &mut known_set);
                    }
                }
            }
        }
    }
}

fn ingest_spidered_hash(state: &AppState, info_hash_hex: &str) -> anyhow::Result<()> {
    // Ensure record exists.
    let mut record = storage::upsert_first_seen(&state.db, info_hash_hex)?;

    // Give it a usable magnet if missing.
    if record
        .magnet
        .as_deref()
        .is_none_or(|m| m.trim().is_empty())
    {
        let magnet = format!("magnet:?xt=urn:btih:{}", info_hash_hex);
        record = storage::set_magnet(&state.db, info_hash_hex, &magnet)?;
    }

    let title = record
        .title
        .clone()
        .unwrap_or_else(|| format!("Torrent {}", &record.info_hash_hex));
    let magnet = record.magnet.clone().unwrap_or_default();

    // Only index "active" torrents to conserve memory.
    // The enrichment worker will update seeders and reindex once >= 2.
    if record.seeders >= 2 {
        state
            .index
            .upsert(&record.info_hash_hex, &title, &magnet, record.seeders)?;
        state.index.maybe_commit().ok();
    }
    Ok(())
}

fn should_accept_hash(seen: &mut RollingBloom, hash: [u8; 20]) -> bool {
    // Fast in-memory dedupe: if we've already seen this hash recently,
    // don't touch the database or index.
    seen.test_and_set(hash)
}

struct RollingBloom {
    current: BloomFilter,
    previous: BloomFilter,
    rotate_every: Duration,
    last_rotate: Instant,
}

impl RollingBloom {
    fn new(bits_pow2: u32, k: u8, rotate_every: Duration) -> Self {
        Self {
            current: BloomFilter::new_pow2(bits_pow2, k),
            previous: BloomFilter::new_pow2(bits_pow2, k),
            rotate_every,
            last_rotate: Instant::now(),
        }
    }

    fn maybe_rotate(&mut self) {
        if self.last_rotate.elapsed() < self.rotate_every {
            return;
        }
        let bits_pow2 = self.current.bits_pow2;
        let k = self.current.k;
        self.previous = std::mem::replace(
            &mut self.current,
            BloomFilter::new_pow2(bits_pow2, k),
        );
        self.last_rotate = Instant::now();
    }

    fn test_and_set(&mut self, hash: [u8; 20]) -> bool {
        self.maybe_rotate();

        // Check both windows first. If either says "seen", we skip the DB.
        if self.current.probably_contains(&hash) || self.previous.probably_contains(&hash) {
            return false;
        }

        // New hash: record it in the current window.
        self.current.insert(&hash);
        true
    }
}

struct BloomFilter {
    bits: Vec<u64>,
    bits_pow2: u32,
    mask: u64,
    k: u8,
}

impl BloomFilter {
    fn new_pow2(bits_pow2: u32, k: u8) -> Self {
        // m = 2^bits_pow2 bits
        let m_bits: usize = 1usize
            .checked_shl(bits_pow2)
            .expect("bits_pow2 too large");
        let words = (m_bits + 63) / 64;
        Self {
            bits: vec![0u64; words],
            bits_pow2,
            mask: (m_bits as u64).saturating_sub(1),
            k: k.max(1),
        }
    }

    #[inline]
    fn probably_contains(&self, item: &[u8; 20]) -> bool {
        let (h1, h2) = bloom_hashes(item);
        for i in 0..self.k {
            let bit_index = h1
                .wrapping_add((i as u64).wrapping_mul(h2))
                & self.mask;
            let word = (bit_index >> 6) as usize;
            let bit = (bit_index & 63) as u32;
            let bitmask = 1u64 << bit;
            if (self.bits[word] & bitmask) == 0 {
                return false;
            }
        }
        true
    }

    #[inline]
    fn insert(&mut self, item: &[u8; 20]) {
        let (h1, h2) = bloom_hashes(item);
        for i in 0..self.k {
            let bit_index = h1
                .wrapping_add((i as u64).wrapping_mul(h2))
                & self.mask;
            let word = (bit_index >> 6) as usize;
            let bit = (bit_index & 63) as u32;
            self.bits[word] |= 1u64 << bit;
        }
    }
}

#[inline]
fn bloom_hashes(item: &[u8; 20]) -> (u64, u64) {
    // Double-hashing scheme: h_i = h1 + i*h2.
    // Make h2 odd to better cover the bitspace.
    let h1 = xxhash_rust::xxh3::xxh3_64(item);
    let h2 = xxhash_rust::xxh3::xxh3_64_with_seed(item, 0x9E37_79B9_7F4A_7C15) | 1;
    (h1, h2)
}

fn push_node(addr: SocketAddr, q: &mut VecDeque<SocketAddr>, set: &mut HashSet<SocketAddr>) {
    if addr.port() == 0 {
        return;
    }
    if !is_publicly_routable_ip(addr.ip()) {
        return;
    }

    if set.insert(addr) {
        q.push_back(addr);
        if q.len() > MAX_KNOWN_NODES {
            if let Some(old) = q.pop_front() {
                set.remove(&old);
            }
        }
    }
}

fn is_publicly_routable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() || v4.is_loopback() || v4.is_unspecified() {
                return false;
            }
            if v4.is_link_local() || v4.is_multicast() {
                return false;
            }

            // Exclude documentation / benchmark ranges.
            let o = v4.octets();
            if (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || (o[0] == 198 && o[1] == 18)
                || (o[0] == 198 && o[1] == 19)
            {
                return false;
            }

            true
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            if v6.is_unique_local() || v6.is_unicast_link_local() {
                return false;
            }

            // 2001:db8::/32 documentation prefix.
            let seg = v6.segments();
            if seg[0] == 0x2001 && seg[1] == 0x0db8 {
                return false;
            }

            true
        }
    }
}

async fn resolve_bootstrap() -> Vec<SocketAddr> {
    let custom = std::env::var("SERMA_SPIDER_BOOTSTRAP")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let mut out = Vec::new();
    let list: Vec<String> = if let Some(s) = custom {
        s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
    } else {
        DEFAULT_BOOTSTRAP.iter().map(|s| s.to_string()).collect()
    };

    for host in list {
        match tokio::net::lookup_host(&host).await {
            Ok(iter) => {
                for addr in iter {
                    out.push(addr);
                }
            }
            Err(err) => {
                tracing::debug!(%err, host=%host, "spider: bootstrap resolve failed");
            }
        }
    }

    out
}

async fn bootstrap_tick(
    socket_v4: Option<&UdpSocket>,
    socket_v6: Option<&UdpSocket>,
    node_id: &[u8; 20],
    known: &mut VecDeque<SocketAddr>,
) {
    // Probe a handful of known nodes each tick.
    for _ in 0..16 {
        let Some(addr) = known.pop_front() else {
            break;
        };
        known.push_back(addr);

        let target = *rbit::peer::PeerId::generate().as_bytes();
        let tx = next_txid();
        let msg = make_find_node(tx, node_id, &target);
        send_to_family(socket_v4, socket_v6, &msg, addr).await;
    }
}

fn next_txid() -> [u8; 2] {
    use std::sync::atomic::{AtomicU16, Ordering};
    static TX: AtomicU16 = AtomicU16::new(0);
    let v = TX.fetch_add(1, Ordering::Relaxed);
    v.to_be_bytes()
}

fn make_find_node(tx: [u8; 2], id: &[u8; 20], target: &[u8; 20]) -> Vec<u8> {
    // d1:ad2:id20:<id>6:target20:<target>e1:q9:find_node1:t2:<tx>1:y1:qe
    let mut out = Vec::with_capacity(110);
    out.push(b'd');

    // "a" dict
    benc_key(&mut out, b"a");
    out.push(b'd');
    benc_key(&mut out, b"id");
    benc_bytes(&mut out, id);
    benc_key(&mut out, b"target");
    benc_bytes(&mut out, target);
    out.push(b'e');

    benc_key(&mut out, b"q");
    benc_bytes(&mut out, b"find_node");

    benc_key(&mut out, b"t");
    benc_bytes(&mut out, &tx);

    benc_key(&mut out, b"y");
    benc_bytes(&mut out, b"q");

    out.push(b'e');
    out
}

fn make_sample_infohashes(tx: [u8; 2], id: &[u8; 20], target: &[u8; 20]) -> Vec<u8> {
    // d1:ad2:id20:<id>6:target20:<target>e1:q17:sample_infohashes1:t2:<tx>1:y1:qe
    let mut out = Vec::with_capacity(140);
    out.push(b'd');

    benc_key(&mut out, b"a");
    out.push(b'd');
    benc_key(&mut out, b"id");
    benc_bytes(&mut out, id);
    benc_key(&mut out, b"target");
    benc_bytes(&mut out, target);
    out.push(b'e');

    benc_key(&mut out, b"q");
    benc_bytes(&mut out, b"sample_infohashes");

    benc_key(&mut out, b"t");
    benc_bytes(&mut out, &tx);

    benc_key(&mut out, b"y");
    benc_bytes(&mut out, b"q");

    out.push(b'e');
    out
}

fn make_response(tx: &[u8], id: &[u8; 20]) -> Vec<u8> {
    // d1:rd2:id20:<id>e1:t<tx>1:y1:re
    let mut out = Vec::with_capacity(80);
    out.push(b'd');

    benc_key(&mut out, b"r");
    out.push(b'd');
    benc_key(&mut out, b"id");
    benc_bytes(&mut out, id);
    out.push(b'e');

    benc_key(&mut out, b"t");
    benc_bytes(&mut out, tx);

    benc_key(&mut out, b"y");
    benc_bytes(&mut out, b"r");

    out.push(b'e');
    out
}

fn benc_key(out: &mut Vec<u8>, key: &[u8]) {
    // Keys must be bytestrings.
    benc_bytes(out, key);
}

fn benc_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    itoa_len(out, bytes.len());
    out.push(b':');
    out.extend_from_slice(bytes);
}

fn itoa_len(out: &mut Vec<u8>, n: usize) {
    // Small integer to ascii.
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut x = n;
    if x == 0 {
        out.push(b'0');
        return;
    }
    while x > 0 {
        i -= 1;
        buf[i] = b'0' + (x % 10) as u8;
        x /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

fn parse_compact_nodes(nodes: &[u8]) -> Vec<SocketAddr> {
    // Compact node info: 26 bytes per node: 20-byte node id + 4-byte IPv4 + 2-byte port.
    let mut out = Vec::new();
    let mut i = 0;
    while i + 26 <= nodes.len() {
        let ip = Ipv4Addr::new(nodes[i + 20], nodes[i + 21], nodes[i + 22], nodes[i + 23]);
        let port = u16::from_be_bytes([nodes[i + 24], nodes[i + 25]]);
        out.push(SocketAddr::new(IpAddr::V4(ip), port));
        i += 26;
    }
    out
}

fn parse_compact_nodes_v6(nodes: &[u8]) -> Vec<SocketAddr> {
    // nodes6: 38 bytes per node: 20-byte node id + 16-byte IPv6 + 2-byte port.
    let mut out = Vec::new();
    let mut i = 0;
    while i + 38 <= nodes.len() {
        let ip = Ipv6Addr::from([
            nodes[i + 20],
            nodes[i + 21],
            nodes[i + 22],
            nodes[i + 23],
            nodes[i + 24],
            nodes[i + 25],
            nodes[i + 26],
            nodes[i + 27],
            nodes[i + 28],
            nodes[i + 29],
            nodes[i + 30],
            nodes[i + 31],
            nodes[i + 32],
            nodes[i + 33],
            nodes[i + 34],
            nodes[i + 35],
        ]);
        let port = u16::from_be_bytes([nodes[i + 36], nodes[i + 37]]);
        out.push(SocketAddr::new(IpAddr::V6(ip), port));
        i += 38;
    }
    out
}

#[derive(Debug)]
struct KrpcMessage<'a> {
    raw: &'a [u8],
}

impl<'a> KrpcMessage<'a> {
    fn decode(raw: &'a [u8]) -> Option<Self> {
        // Quick sanity: must be a dictionary.
        if raw.first().copied()? != b'd' {
            return None;
        }
        Some(Self { raw })
    }

    fn is_query(&self) -> bool {
        benc_get_bytes(self.raw, b"y").is_some_and(|v| v == b"q")
    }

    fn is_response(&self) -> bool {
        benc_get_bytes(self.raw, b"y").is_some_and(|v| v == b"r")
    }

    fn compact_nodes(&self) -> Option<&'a [u8]> {
        // Look for r:nodes in responses.
        let r = benc_get_dict(self.raw, b"r")?;
        benc_get_bytes(r, b"nodes")
    }

    fn compact_nodes_v6(&self) -> Option<&'a [u8]> {
        // Look for r:nodes6 in responses.
        let r = benc_get_dict(self.raw, b"r")?;
        benc_get_bytes(r, b"nodes6")
    }

    fn samples_from_response(&self) -> Option<&'a [u8]> {
        if !self.is_response() {
            return None;
        }
        let r = benc_get_dict(self.raw, b"r")?;
        benc_get_bytes(r, b"samples")
    }

    fn info_hash_from_query(&self) -> Option<[u8; 20]> {
        if !self.is_query() {
            return None;
        }
        let q = benc_get_bytes(self.raw, b"q")?;
        if q != b"announce_peer" && q != b"get_peers" {
            return None;
        }
        let a = benc_get_dict(self.raw, b"a")?;
        let info = benc_get_bytes(a, b"info_hash")?;
        if info.len() != 20 {
            return None;
        }
        let mut out = [0u8; 20];
        out.copy_from_slice(info);
        Some(out)
    }

    fn make_minimal_response(&self, node_id: &[u8; 20]) -> Option<Vec<u8>> {
        if !self.is_query() {
            return None;
        }
        let tx = benc_get_bytes(self.raw, b"t")?;
        Some(make_response(tx, node_id))
    }
}

async fn sample_tick(
    socket_v4: Option<&UdpSocket>,
    socket_v6: Option<&UdpSocket>,
    node_id: &[u8; 20],
    known: &mut VecDeque<SocketAddr>,
) {
    // Query a handful of known nodes for hash samples (BEP-51).
    for _ in 0..SAMPLE_PER_TICK {
        let Some(addr) = known.pop_front() else {
            break;
        };
        known.push_back(addr);

        let target = *rbit::peer::PeerId::generate().as_bytes();
        let tx = next_txid();
        let msg = make_sample_infohashes(tx, node_id, &target);
        send_to_family(socket_v4, socket_v6, &msg, addr).await;
    }
}

async fn send_to_family(
    socket_v4: Option<&UdpSocket>,
    socket_v6: Option<&UdpSocket>,
    msg: &[u8],
    addr: SocketAddr,
) {
    match addr.ip() {
        IpAddr::V4(_) => {
            if let Some(sock) = socket_v4 {
                let _ = sock.send_to(msg, addr).await;
            }
        }
        IpAddr::V6(_) => {
            if let Some(sock) = socket_v6 {
                let _ = sock.send_to(msg, addr).await;
            }
        }
    }
}

async fn recv_from_any(
    socket_v4: Option<&UdpSocket>,
    socket_v6: Option<&UdpSocket>,
    buf4: &mut [u8],
    buf6: &mut [u8],
) -> Option<(usize, SocketAddr, u8)> {
    tokio::select! {
        r = async {
            if let Some(sock) = socket_v4 {
                sock.recv_from(buf4).await
            } else {
                std::future::pending::<std::io::Result<(usize, SocketAddr)>>().await
            }
        } => r.ok().map(|(n, from)| (n, from, 4u8)),
        r = async {
            if let Some(sock) = socket_v6 {
                sock.recv_from(buf6).await
            } else {
                std::future::pending::<std::io::Result<(usize, SocketAddr)>>().await
            }
        } => r.ok().map(|(n, from)| (n, from, 6u8)),
    }
}

// ------------------------------
// Minimal bencode “dict-getter”
// ------------------------------

fn benc_get_bytes<'a>(raw: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let dict = BencParser::new(raw).parse_dict()?;
    dict.get_bytes(key)
}

fn benc_get_dict<'a>(raw: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let dict = BencParser::new(raw).parse_dict()?;
    dict.get_dict_slice(key)
}

struct BencDict<'a> {
    // Slice containing the dict payload (starts at 'd', ends at matching 'e').
    raw: &'a [u8],
}

impl<'a> BencDict<'a> {
    fn get_bytes(&self, key: &[u8]) -> Option<&'a [u8]> {
        let mut p = BencParser::new(self.raw);
        p.expect_byte(b'd')?;
        loop {
            if p.peek()? == b'e' {
                return None;
            }
            let k = p.parse_bytes()?;
            let v_start = p.pos;
            match p.peek()? {
                b'd' => {
                    // Skip dict value
                    p.skip_value()?;
                    let v_end = p.pos;
                    if k == key {
                        // For bytes, we don't want dict.
                        let _ = (v_start, v_end);
                        return None;
                    }
                }
                b'l' | b'i' | b'0'..=b'9' => {
                    let bytes = p.parse_value_as_bytes_if_bytestring()?;
                    if k == key {
                        return bytes;
                    }
                }
                _ => return None,
            }
        }
    }

    fn get_dict_slice(&self, key: &[u8]) -> Option<&'a [u8]> {
        let mut p = BencParser::new(self.raw);
        p.expect_byte(b'd')?;
        loop {
            if p.peek()? == b'e' {
                return None;
            }
            let k = p.parse_bytes()?;
            let v_start = p.pos;
            if p.peek()? != b'd' {
                p.skip_value()?;
                continue;
            }
            p.skip_value()?; // skip dict
            let v_end = p.pos;
            if k == key {
                return self.raw.get(v_start..v_end);
            }
        }
    }
}

struct BencParser<'a> {
    raw: &'a [u8],
    pos: usize,
}

impl<'a> BencParser<'a> {
    fn new(raw: &'a [u8]) -> Self {
        Self { raw, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.raw.get(self.pos).copied()
    }

    fn expect_byte(&mut self, b: u8) -> Option<()> {
        if self.peek()? != b {
            return None;
        }
        self.pos += 1;
        Some(())
    }

    fn parse_dict(mut self) -> Option<BencDict<'a>> {
        // Return the slice spanning the whole dict.
        if self.peek()? != b'd' {
            return None;
        }
        let start = self.pos;
        self.skip_value()?;
        let end = self.pos;
        Some(BencDict {
            raw: self.raw.get(start..end)?,
        })
    }

    fn parse_bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.parse_usize()?;
        self.expect_byte(b':')?;
        let start = self.pos;
        let end = self.pos.checked_add(len)?;
        let out = self.raw.get(start..end)?;
        self.pos = end;
        Some(out)
    }

    fn parse_usize(&mut self) -> Option<usize> {
        let mut n: usize = 0;
        let mut saw = false;
        while let Some(b) = self.peek() {
            if !b.is_ascii_digit() {
                break;
            }
            saw = true;
            n = n.checked_mul(10)? + (b - b'0') as usize;
            self.pos += 1;
        }
        if !saw {
            None
        } else {
            Some(n)
        }
    }

    fn parse_value_as_bytes_if_bytestring(&mut self) -> Option<Option<&'a [u8]>> {
        match self.peek()? {
            b'0'..=b'9' => self.parse_bytes().map(Some),
            _ => {
                self.skip_value()?;
                Some(None)
            }
        }
    }

    fn skip_value(&mut self) -> Option<()> {
        match self.peek()? {
            b'i' => {
                self.pos += 1;
                while self.peek()? != b'e' {
                    self.pos += 1;
                    if self.pos >= self.raw.len() {
                        return None;
                    }
                }
                self.pos += 1;
                Some(())
            }
            b'l' => {
                self.pos += 1;
                while self.peek()? != b'e' {
                    self.skip_value()?;
                }
                self.pos += 1;
                Some(())
            }
            b'd' => {
                self.pos += 1;
                while self.peek()? != b'e' {
                    self.parse_bytes()?; // key
                    self.skip_value()?;  // value
                }
                self.pos += 1;
                Some(())
            }
            b'0'..=b'9' => {
                let len = self.parse_usize()?;
                self.expect_byte(b':')?;
                self.pos = self.pos.checked_add(len)?;
                if self.pos > self.raw.len() {
                    return None;
                }
                Some(())
            }
            _ => None,
        }
    }
}
