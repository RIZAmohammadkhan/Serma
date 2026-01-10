use crate::{AppState, storage};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::time::{Duration, interval};

// Minimal BEP-5 DHT “spider”:
// - Joins the DHT via bootstrap nodes (find_node)
// - Responds to incoming queries so other nodes keep us in their routing tables
// - Harvests info_hash from announce_peer / get_peers queries
// - Learns more nodes from responses (“nodes” compact format)

const DEFAULT_BIND: &str = "0.0.0.0:6881";
const DEFAULT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
];

const MAX_KNOWN_NODES: usize = 10_000;
const MAX_SEEN_HASHES: usize = 50_000;
const SEEN_TTL: Duration = Duration::from_secs(30 * 60);

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

    let socket = match UdpSocket::bind(&bind).await {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(%err, bind = %bind, "spider: failed to bind; trying ephemeral port");
            match UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => s,
                Err(err) => {
                    tracing::warn!(%err, "spider: failed to bind UDP socket; spider disabled");
                    return;
                }
            }
        }
    };

    let local_addr = match socket.local_addr() {
        Ok(a) => a,
        Err(err) => {
            tracing::warn!(%err, "spider: failed to get local addr");
            return;
        }
    };

    // DHT node id: 20 random-ish bytes. We reuse rbit’s peer-id generator (also 20 bytes).
    let node_id = *rbit::peer::PeerId::generate().as_bytes();

    tracing::info!(bind = %local_addr, "spider: listening");

    let mut known_nodes: VecDeque<SocketAddr> = VecDeque::new();
    let mut known_set: HashSet<SocketAddr> = HashSet::new();

    let mut seen_hashes: HashMap<[u8; 20], Instant> = HashMap::new();

    // Bootstrap right away.
    for addr in resolve_bootstrap().await {
        push_node(addr, &mut known_nodes, &mut known_set);
    }
    bootstrap_tick(&socket, &node_id, &mut known_nodes).await;

    let mut boot_int = interval(Duration::from_secs(15));
    let mut gc_int = interval(Duration::from_secs(30));

    let mut buf = vec![0u8; 4096];
    loop {
        tokio::select! {
            _ = boot_int.tick() => {
                bootstrap_tick(&socket, &node_id, &mut known_nodes).await;
            }
            _ = gc_int.tick() => {
                gc_seen(&mut seen_hashes);
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
            recv = socket.recv_from(&mut buf) => {
                let Ok((n, from)) = recv else {
                    continue;
                };
                if n == 0 {
                    continue;
                }

                if let Some(msg) = KrpcMessage::decode(&buf[..n]) {
                    // Learn nodes from responses.
                    if let Some(nodes) = msg.compact_nodes() {
                        for addr in parse_compact_nodes(nodes) {
                            push_node(addr, &mut known_nodes, &mut known_set);
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
                            let _ = socket.send_to(&resp, from).await;
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

    state.index.upsert(&record.info_hash_hex, &title, &magnet, record.seeders)?;
    state.index.maybe_commit().ok();
    Ok(())
}

fn should_accept_hash(seen: &mut HashMap<[u8; 20], Instant>, hash: [u8; 20]) -> bool {
    let now = Instant::now();
    if let Some(prev) = seen.get(&hash) {
        if now.duration_since(*prev) < SEEN_TTL {
            return false;
        }
    }
    if seen.len() >= MAX_SEEN_HASHES {
        // Best-effort eviction: drop some oldest-ish entries by TTL sweep.
        gc_seen(seen);
        if seen.len() >= MAX_SEEN_HASHES {
            // Still huge; just clear to avoid unbounded growth.
            seen.clear();
        }
    }
    seen.insert(hash, now);
    true
}

fn gc_seen(seen: &mut HashMap<[u8; 20], Instant>) {
    let now = Instant::now();
    seen.retain(|_, t| now.duration_since(*t) < SEEN_TTL);
}

fn push_node(addr: SocketAddr, q: &mut VecDeque<SocketAddr>, set: &mut HashSet<SocketAddr>) {
    if addr.port() == 0 {
        return;
    }
    // Avoid obviously-useless addresses.
    match addr.ip() {
        IpAddr::V4(v4) => {
            if v4.is_private() || v4.is_loopback() || v4.is_unspecified() {
                return;
            }
        }
        IpAddr::V6(_) => {
            // Keep it simple for MVP; ignore v6.
            return;
        }
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

async fn bootstrap_tick(socket: &UdpSocket, node_id: &[u8; 20], known: &mut VecDeque<SocketAddr>) {
    // Probe a handful of known nodes each tick.
    for _ in 0..16 {
        let Some(addr) = known.pop_front() else {
            break;
        };
        known.push_back(addr);

        let target = *rbit::peer::PeerId::generate().as_bytes();
        let tx = next_txid();
        let msg = make_find_node(tx, node_id, &target);
        let _ = socket.send_to(&msg, addr).await;
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

    fn compact_nodes(&self) -> Option<&'a [u8]> {
        // Look for r:nodes in responses.
        let r = benc_get_dict(self.raw, b"r")?;
        benc_get_bytes(r, b"nodes")
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
