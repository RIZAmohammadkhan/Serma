use crate::{config::Config, AppState, storage};
use anyhow::Context;
use base64::Engine as _;
use bytes::Bytes;
use rbit::bencode;
use rbit::metainfo::{InfoHash, MagnetLink};
use rbit::peer::{
    ExtensionHandshake, ExtensionMessage, METADATA_PIECE_SIZE, Message, MetadataMessage,
    MetadataMessageType, PeerConnection, PeerId, metadata_piece_size,
};
use rbit::tracker::{AnnounceParams, TrackerClient, TrackerEvent};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Semaphore;
use tokio::time::{Duration, timeout};

use crate::socks5::{Socks5Config, Socks5UdpAssociate};

pub async fn run(state: AppState) {
    let tracker = Arc::new(TrackerClient::new());
    let sem = Arc::new(Semaphore::new(state.config.enrich_max_concurrent));

    loop {
        let missing = match storage::list_missing_info(&state.db, state.config.enrich_missing_scan_limit) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(%err, "enrich: failed scanning sled");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        if missing.is_empty() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        for record in missing {
            let permit = match sem.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };

            let state = state.clone();
            let tracker = tracker.clone();

            tokio::spawn(async move {
                let _permit = permit;
                if let Err(err) = enrich_one(&state, &tracker, record).await {
                    tracing::debug!(%err, "enrich: failed");
                }
            });
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn enrich_one(
    state: &AppState,
    tracker: &TrackerClient,
    record: storage::TorrentRecord,
) -> anyhow::Result<()> {
    let info_hash_bytes = parse_info_hash_hex(&record.info_hash_hex)
        .with_context(|| format!("invalid info hash: {}", record.info_hash_hex))?;

    tracing::debug!(hash = %record.info_hash_hex, "enrich: start");

    let peers = timeout(
        Duration::from_secs(state.config.enrich_dht_get_peers_timeout_secs),
        dht_get_peers_krpc(&state.config, info_hash_bytes),
    )
        .await
        .context("dht get_peers timed out")??;

    tracing::debug!(hash = %record.info_hash_hex, peers = peers.len(), "enrich: dht peers");

    if peers.is_empty() {
        return Ok(());
    }

    // Best-effort: use DHT peer count as a lower-bound popularity signal.
    // Cap it to avoid writing unrealistic values.
    // (Trackers provide real seeder counts when available.)
    let dht_peers_lb = (peers.len().min(50)) as i64;
    if dht_peers_lb > record.seeders {
        let _ = storage::set_seeders(&state.db, &record.info_hash_hex, dht_peers_lb);
    }

    // Try multiple peers concurrently; many peers will refuse connections or lack ut_metadata.
    // Concurrency keeps enrichment from stalling on slow/blocked peers.
    let max_metadata_inflight = state.config.enrich_metadata_inflight;
    let metadata_overall_timeout = Duration::from_secs(state.config.enrich_metadata_overall_timeout_secs);

    let mut tried: usize = 0;
    let mut failures_logged: usize = 0;
    let mut last_err: Option<anyhow::Error> = None;

    let mut join_set = tokio::task::JoinSet::new();
    let mut peer_iter = peers.into_iter().take(state.config.enrich_peers_per_hash);
    for _ in 0..max_metadata_inflight {
        if let Some(peer) = peer_iter.next() {
            tried += 1;
            join_set.spawn(async move {
                let r = timeout(metadata_overall_timeout, fetch_ut_metadata(peer, info_hash_bytes)).await;
                (peer, r)
            });
        }
    }

    let mut metadata: Option<Vec<u8>> = None;
    while let Some(joined) = join_set.join_next().await {
        let (peer, result) = match joined {
            Ok(v) => v,
            Err(err) => {
                last_err = Some(anyhow::anyhow!("metadata task join error: {err}"));
                continue;
            }
        };

        match result {
            Ok(Ok(info_bytes)) => {
                tracing::debug!(hash = %record.info_hash_hex, peer = %peer, bytes = info_bytes.len(), "enrich: got metadata");
                metadata = Some(info_bytes);
                join_set.abort_all();
                break;
            }
            Ok(Err(err)) => {
                last_err = Some(err);
                if failures_logged < 2 {
                    if let Some(err) = last_err.as_ref() {
                        tracing::debug!(hash = %record.info_hash_hex, peer = %peer, err = %err, "enrich: peer failed");
                    }
                    failures_logged += 1;
                } else {
                    if let Some(err) = last_err.as_ref() {
                        tracing::trace!(hash = %record.info_hash_hex, peer = %peer, err = %err, "enrich: peer failed");
                    }
                }
            }
            Err(_elapsed) => {
                last_err = Some(anyhow::anyhow!("metadata fetch timed out"));
                if failures_logged < 2 {
                    tracing::debug!(hash = %record.info_hash_hex, peer = %peer, "enrich: peer failed: metadata fetch timed out");
                    failures_logged += 1;
                } else {
                    tracing::trace!(hash = %record.info_hash_hex, peer = %peer, "enrich: peer failed: metadata fetch timed out");
                }
            }
        }

        if let Some(next_peer) = peer_iter.next() {
            tried += 1;
            join_set.spawn(async move {
                let r = timeout(metadata_overall_timeout, fetch_ut_metadata(next_peer, info_hash_bytes)).await;
                (next_peer, r)
            });
        } else if join_set.is_empty() {
            break;
        }
    }

    let Some(info_bytes) = metadata else {
        if let Some(err) = last_err {
            tracing::debug!(hash = %record.info_hash_hex, tried, err = %err, "enrich: metadata unavailable");
        } else {
            tracing::debug!(hash = %record.info_hash_hex, tried, "enrich: metadata unavailable");
        }
        return Ok(());
    };

    let title = extract_name_from_info(&info_bytes).ok();
    let info_b64 = base64::engine::general_purpose::STANDARD.encode(&info_bytes);
    let mut updated = storage::set_metadata(
        &state.db,
        &record.info_hash_hex,
        title.as_deref(),
        &info_b64,
    )?;

    // If the record contains trackers in its magnet, use them to get a real seeder count.
    if let Some(magnet) = updated.magnet.clone() {
        if let Ok(m) = MagnetLink::parse(&magnet) {
            if !m.trackers.is_empty() {
                if let Ok(hash) = InfoHash::from_hex(&updated.info_hash_hex) {
                    let peer_id = *PeerId::generate().as_bytes();
                    if let Some(seeders) =
                        announce_seeders(tracker, &hash, &peer_id, &m.trackers).await
                    {
                        if seeders > updated.seeders {
                            updated =
                                storage::set_seeders(&state.db, &updated.info_hash_hex, seeders)?;
                        }
                    }
                }
            }
        }
    }

    let title_for_index = updated
        .title
        .clone()
        .unwrap_or_else(|| format!("Torrent {}", &updated.info_hash_hex));
    let magnet_for_index = updated.magnet.clone().unwrap_or_default();

    // Only index torrents that meet the minimum activity threshold.
    if updated.seeders >= 2 {
        let _ = state.index.upsert(
            &updated.info_hash_hex,
            &title_for_index,
            &magnet_for_index,
            updated.seeders,
        );
    } else {
        // If it was previously indexed, remove it.
        let _ = state.index.delete(&updated.info_hash_hex);
    }
    let _ = state.index.maybe_commit();
    Ok(())
}

async fn dht_get_peers_krpc(cfg: &Config, info_hash: [u8; 20]) -> anyhow::Result<Vec<SocketAddr>> {
    let transport = match Socks5Config::from_env() {
        Some(Ok(cfg)) => {
            let sock = Socks5UdpAssociate::connect(&cfg)
                .await
                .with_context(|| format!("enrich: connect SOCKS5 proxy {}", cfg.proxy))?;
            DhtTransport::Socks { sock }
        }
        Some(Err(err)) => {
            anyhow::bail!("enrich: invalid SERMA_SOCKS5_PROXY: {err}");
        }
        None => {
            // Use separate IPv4 + (optional) IPv6 UDP sockets so we can talk to both
            // families regardless of OS IPv6 dual-stack settings.
            let socket_v4 = UdpSocket::bind("0.0.0.0:0").await?;
            let socket_v6 = match UdpSocket::bind("[::]:0").await {
                Ok(s) => Some(s),
                Err(err) => {
                    tracing::debug!(%err, "enrich: ipv6 udp bind failed; continuing with ipv4 only");
                    None
                }
            };
            DhtTransport::Direct { socket_v4, socket_v6 }
        }
    };
    let node_id = *PeerId::generate().as_bytes();

    let bootstrap = resolve_bootstrap(cfg).await;
    if bootstrap.is_empty() {
        anyhow::bail!("no DHT bootstrap nodes resolved");
    }

    // Prefer nodes closer (XOR-distance) to the target infohash.
    // Store a min-heap by using Reverse(distance).
    let mut q: BinaryHeap<(Reverse<[u8; 20]>, SocketAddr)> = BinaryHeap::new();
    let mut seen_nodes: HashSet<SocketAddr> = HashSet::new();
    for addr in bootstrap {
        push_node_seed(addr, &mut q, &mut seen_nodes);
    }

    let mut peers: Vec<SocketAddr> = Vec::new();
    let mut seen_peers: HashSet<SocketAddr> = HashSet::new();
    let mut tx: u16 = 0;
    let mut buf4 = vec![0u8; 4096];
    let mut buf6 = vec![0u8; 4096];
    let mut queries = 0usize;

    // Track a small window of in-flight queries so we don't miss responses due to timing.
    // key=txid, value=(addr, sent_at)
    let mut inflight: HashMap<[u8; 2], (SocketAddr, tokio::time::Instant)> = HashMap::new();
    let max_inflight: usize = cfg.enrich_dht_inflight;

    // Bound total time spent per hash lookup (outer timeout still applies too).
    let overall_deadline = tokio::time::Instant::now()
        + Duration::from_secs(cfg.enrich_dht_overall_deadline_secs);

    while tokio::time::Instant::now() < overall_deadline {
        if peers.len() >= cfg.enrich_peers_per_hash {
            break;
        }
        if queries >= cfg.enrich_dht_max_queries_per_hash {
            break;
        }

        // Reap timed-out inflight requests.
        let now = tokio::time::Instant::now();
        let query_timeout = Duration::from_millis(cfg.enrich_dht_query_timeout_ms);
        inflight.retain(|_, (_, sent_at)| now.saturating_duration_since(*sent_at) <= query_timeout);

        // Fill the inflight window.
        while inflight.len() < max_inflight
            && queries < cfg.enrich_dht_max_queries_per_hash
            && peers.len() < cfg.enrich_peers_per_hash
        {
            let Some((_, addr)) = q.pop() else { break };
            tx = tx.wrapping_add(1);
            let txid = tx.to_be_bytes();
            let msg = make_get_peers(txid, &node_id, &info_hash);
            let _ = dht_send(&transport, &msg, addr).await;
            inflight.insert(txid, (addr, tokio::time::Instant::now()));
            queries += 1;
        }

        if inflight.is_empty() && q.is_empty() {
            break;
        }

        // Process responses; use a short receive timeout to keep the loop responsive.
        let recv = dht_recv(
            &transport,
            &mut buf4,
            &mut buf6,
            Duration::from_millis(cfg.enrich_dht_recv_timeout_ms),
        );
        let Some((n_res, fam_tag)) = recv.await else {
            continue;
        };
        let Ok(n) = n_res else {
            continue;
        };
        if n == 0 {
            continue;
        }

        let raw = if fam_tag == 4 { &buf4[..n] } else { &buf6[..n] };
        let Some(resp) = KrpcResponse::decode(raw) else {
            continue;
        };

        // Only accept responses for txids we sent.
        if inflight.remove(&resp.tx).is_none() {
            continue;
        }

        if let Some(nodes) = resp.nodes {
            for node in parse_compact_nodes_v4(nodes) {
                push_node(node, &info_hash, &mut q, &mut seen_nodes);
            }
        }
        if let Some(nodes6) = resp.nodes6 {
            for node in parse_compact_nodes_v6(nodes6) {
                push_node(node, &info_hash, &mut q, &mut seen_nodes);
            }
        }
        if let Some(values) = resp.values {
            for v in values {
                if let Some(peer) = parse_compact_peer_v4(&v) {
                    if seen_peers.insert(peer) {
                        peers.push(peer);
                        if peers.len() >= cfg.enrich_peers_per_hash {
                            break;
                        }
                    }
                }
            }
        }
        if let Some(values6) = resp.values6 {
            for v in values6 {
                if let Some(peer) = parse_compact_peer_v6(&v) {
                    if seen_peers.insert(peer) {
                        peers.push(peer);
                        if peers.len() >= cfg.enrich_peers_per_hash {
                            break;
                        }
                    }
                }
            }
        }
    }

    Ok(peers)
}

enum DhtTransport {
    Direct {
        socket_v4: UdpSocket,
        socket_v6: Option<UdpSocket>,
    },
    Socks {
        sock: Socks5UdpAssociate,
    },
}

async fn dht_send(
    transport: &DhtTransport,
    msg: &[u8],
    addr: SocketAddr,
) -> std::io::Result<usize> {
    match transport {
        DhtTransport::Direct { socket_v4, socket_v6 } => match addr.ip() {
            IpAddr::V4(_) => socket_v4.send_to(msg, addr).await,
            IpAddr::V6(_) => {
                if let Some(sock6) = socket_v6.as_ref() {
                    sock6.send_to(msg, addr).await
                } else {
                    Ok(0)
                }
            }
        },
        DhtTransport::Socks { sock } => sock.send_to(msg, addr).await,
    }
}

async fn dht_recv(
    transport: &DhtTransport,
    buf4: &mut [u8],
    buf6: &mut [u8],
    per_recv_timeout: Duration,
) -> Option<(std::io::Result<usize>, u8)> {
    let sleep = tokio::time::sleep(per_recv_timeout);
    tokio::pin!(sleep);

    match transport {
        DhtTransport::Direct { socket_v4, socket_v6 } => {
            tokio::select! {
                _ = &mut sleep => None,
                r = socket_v4.recv_from(buf4) => Some((r.map(|(n, _)| n), 4u8)),
                r = async {
                    if let Some(sock6) = socket_v6.as_ref() {
                        sock6.recv_from(buf6).await.map(|(n, _)| n)
                    } else {
                        // Never resolves when IPv6 socket is unavailable.
                        std::future::pending::<std::io::Result<usize>>().await
                    }
                } => Some((r, 6u8)),
            }
        }
        DhtTransport::Socks { sock } => {
            tokio::select! {
                _ = &mut sleep => None,
                r = sock.recv_from(buf4) => {
                    Some((r.map(|(n, from)| {
                        // Place bytes in buf4; caller uses fam_tag to choose buf.
                        let _ = from;
                        n
                    }), 4u8))
                }
            }
        }
    }
}

async fn resolve_bootstrap(cfg: &Config) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for host in cfg.enrich_dht_bootstrap.iter().cloned() {
        match tokio::net::lookup_host(&host).await {
            Ok(iter) => {
                for addr in iter {
                    out.push(addr);
                }
            }
            Err(err) => {
                tracing::debug!(%err, host=%host, "enrich: bootstrap resolve failed");
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct DhtNode {
    id: [u8; 20],
    addr: SocketAddr,
}

fn xor_distance(a: &[u8; 20], b: &[u8; 20]) -> [u8; 20] {
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = a[i] ^ b[i];
    }
    out
}

fn push_node_seed(
    addr: SocketAddr,
    q: &mut BinaryHeap<(Reverse<[u8; 20]>, SocketAddr)>,
    set: &mut HashSet<SocketAddr>,
) {
    // Seed nodes don't come with an ID, so just give them highest priority.
    if !filter_addr(addr) {
        return;
    }
    if set.insert(addr) {
        q.push((Reverse([0u8; 20]), addr));
    }
}

fn push_node(
    node: DhtNode,
    target: &[u8; 20],
    q: &mut BinaryHeap<(Reverse<[u8; 20]>, SocketAddr)>,
    set: &mut HashSet<SocketAddr>,
) {
    if !filter_addr(node.addr) {
        return;
    }
    if set.insert(node.addr) {
        let dist = xor_distance(&node.id, target);
        q.push((Reverse(dist), node.addr));
    }
}

fn filter_addr(addr: SocketAddr) -> bool {
    if addr.port() == 0 {
        return false;
    }
    match addr.ip() {
        IpAddr::V4(v4) => {
            if v4.is_private() || v4.is_loopback() || v4.is_unspecified() {
                return false;
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local() {
                return false;
            }
        }
    }
    true
}

fn parse_compact_peer_v4(bytes: &[u8]) -> Option<SocketAddr> {
    if bytes.len() != 6 {
        return None;
    }
    let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    let port = u16::from_be_bytes([bytes[4], bytes[5]]);
    Some(SocketAddr::new(IpAddr::V4(ip), port))
}

fn parse_compact_peer_v6(bytes: &[u8]) -> Option<SocketAddr> {
    if bytes.len() != 18 {
        return None;
    }
    let ip = Ipv6Addr::from([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    let port = u16::from_be_bytes([bytes[16], bytes[17]]);
    Some(SocketAddr::new(IpAddr::V6(ip), port))
}

fn parse_compact_nodes_v4(nodes: &[u8]) -> Vec<DhtNode> {
    // Compact node info: 26 bytes per node: 20-byte node id + 4-byte IPv4 + 2-byte port.
    let mut out = Vec::new();
    let mut i = 0;
    while i + 26 <= nodes.len() {
        let mut id = [0u8; 20];
        id.copy_from_slice(&nodes[i..i + 20]);
        let ip = Ipv4Addr::new(nodes[i + 20], nodes[i + 21], nodes[i + 22], nodes[i + 23]);
        let port = u16::from_be_bytes([nodes[i + 24], nodes[i + 25]]);
        out.push(DhtNode {
            id,
            addr: SocketAddr::new(IpAddr::V4(ip), port),
        });
        i += 26;
    }
    out
}

fn parse_compact_nodes_v6(nodes: &[u8]) -> Vec<DhtNode> {
    // nodes6: 38 bytes per node: 20-byte node id + 16-byte IPv6 + 2-byte port.
    let mut out = Vec::new();
    let mut i = 0;
    while i + 38 <= nodes.len() {
        let mut id = [0u8; 20];
        id.copy_from_slice(&nodes[i..i + 20]);
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
        out.push(DhtNode {
            id,
            addr: SocketAddr::new(IpAddr::V6(ip), port),
        });
        i += 38;
    }
    out
}

fn make_get_peers(tx: [u8; 2], id: &[u8; 20], info_hash: &[u8; 20]) -> Vec<u8> {
    // d1:ad2:id20:<id>9:info_hash20:<info>e1:q9:get_peers1:t2:<tx>1:y1:qe
    let mut out = Vec::with_capacity(120);
    out.push(b'd');

    benc_key(&mut out, b"a");
    out.push(b'd');
    benc_key(&mut out, b"id");
    benc_bytes(&mut out, id);
    benc_key(&mut out, b"info_hash");
    benc_bytes(&mut out, info_hash);
    out.push(b'e');

    benc_key(&mut out, b"q");
    benc_bytes(&mut out, b"get_peers");

    benc_key(&mut out, b"t");
    benc_bytes(&mut out, &tx);

    benc_key(&mut out, b"y");
    benc_bytes(&mut out, b"q");

    out.push(b'e');
    out
}

fn benc_key(out: &mut Vec<u8>, key: &[u8]) {
    benc_bytes(out, key);
}

fn benc_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    itoa_len(out, bytes.len());
    out.push(b':');
    out.extend_from_slice(bytes);
}

fn itoa_len(out: &mut Vec<u8>, n: usize) {
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

struct KrpcResponse<'a> {
    tx: [u8; 2],
    nodes: Option<&'a [u8]>,
    nodes6: Option<&'a [u8]>,
    values: Option<Vec<Vec<u8>>>,
    values6: Option<Vec<Vec<u8>>>,
}

impl<'a> KrpcResponse<'a> {
    fn decode(raw: &'a [u8]) -> Option<Self> {
        if raw.first().copied()? != b'd' {
            return None;
        }
        let y = benc_get_bytes(raw, b"y")?;
        if y != b"r" {
            return None;
        }
        let t = benc_get_bytes(raw, b"t")?;
        if t.len() != 2 {
            return None;
        }
        let mut tx = [0u8; 2];
        tx.copy_from_slice(t);

        let r = benc_get_dict(raw, b"r")?;
        let nodes = benc_get_bytes(r, b"nodes");
        let nodes6 = benc_get_bytes(r, b"nodes6");
        let values = benc_get_list_bytes(r, b"values");
        let values6 = benc_get_list_bytes(r, b"values6");

        Some(Self {
            tx,
            nodes,
            nodes6,
            values,
            values6,
        })
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

fn benc_get_list_bytes(raw: &[u8], key: &[u8]) -> Option<Vec<Vec<u8>>> {
    let dict = BencParser::new(raw).parse_dict()?;
    dict.get_list_bytes(key)
}

struct BencDict<'a> {
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
            match p.peek()? {
                b'0'..=b'9' => {
                    let bytes = p.parse_bytes()?;
                    if k == key {
                        return Some(bytes);
                    }
                }
                b'd' | b'l' | b'i' => {
                    p.skip_value()?;
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
            p.skip_value()?;
            let v_end = p.pos;
            if k == key {
                return self.raw.get(v_start..v_end);
            }
        }
    }

    fn get_list_bytes(&self, key: &[u8]) -> Option<Vec<Vec<u8>>> {
        let mut p = BencParser::new(self.raw);
        p.expect_byte(b'd')?;
        loop {
            if p.peek()? == b'e' {
                return None;
            }
            let k = p.parse_bytes()?;
            if p.peek()? != b'l' {
                p.skip_value()?;
                continue;
            }

            // List value.
            p.expect_byte(b'l')?;
            let mut out: Vec<Vec<u8>> = Vec::new();
            while p.peek()? != b'e' {
                match p.peek()? {
                    b'0'..=b'9' => {
                        let b = p.parse_bytes()?;
                        out.push(b.to_vec());
                    }
                    _ => {
                        p.skip_value()?;
                    }
                }
            }
            p.expect_byte(b'e')?;
            if k == key {
                return Some(out);
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
        if !saw { None } else { Some(n) }
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
                    self.parse_bytes()?;
                    self.skip_value()?;
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

async fn fetch_ut_metadata(addr: SocketAddr, info_hash: [u8; 20]) -> anyhow::Result<Vec<u8>> {
    let peer_id = *PeerId::generate().as_bytes();
    let mut conn = timeout(
        Duration::from_secs(6),
        PeerConnection::connect(addr, info_hash, peer_id),
    )
    .await
    .context("peer connect timed out")??;

    if !conn.supports_extension {
        anyhow::bail!("peer does not support BEP-10");
    }

    let mut hs = ExtensionHandshake::with_extensions(&[("ut_metadata", 1)]);
    hs.client = Some("serma".to_string());

    let payload = hs.encode()?;
    conn.send(Message::Extended { id: 0, payload }).await?;

    let (ut_metadata_id, mut total_size) = wait_for_peer_handshake(&mut conn).await?;

    // If peer didn't advertise metadata_size, we still can request piece 0 to learn total_size.
    if total_size.is_none() {
        request_piece(&mut conn, ut_metadata_id, 0).await?;
        let msg = recv_metadata_msg(&mut conn, ut_metadata_id, Duration::from_secs(6)).await?;
        if msg.msg_type != MetadataMessageType::Data {
            anyhow::bail!("peer did not send metadata data for piece 0");
        }
        total_size = msg.total_size;
    }

    let total_size = total_size.context("missing metadata total_size")? as usize;
    let piece_count = (total_size + (METADATA_PIECE_SIZE - 1)) / METADATA_PIECE_SIZE;
    if piece_count == 0 {
        anyhow::bail!("metadata has zero pieces");
    }

    // Request all pieces.
    for piece in 0..piece_count {
        request_piece(&mut conn, ut_metadata_id, piece as u32).await?;
    }

    let mut pieces: Vec<Option<Bytes>> = vec![None; piece_count];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while pieces.iter().any(|p| p.is_none()) {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timed out waiting for metadata pieces");
        }
        let remaining = deadline - now;
        let msg = recv_metadata_msg(&mut conn, ut_metadata_id, remaining).await?;
        if msg.msg_type == MetadataMessageType::Reject {
            anyhow::bail!("peer rejected metadata piece {}", msg.piece);
        }
        if msg.msg_type != MetadataMessageType::Data {
            continue;
        }
        let Some(data) = msg.data else {
            continue;
        };
        let idx = msg.piece as usize;
        if idx < pieces.len() {
            pieces[idx] = Some(data);
        }
    }

    // Assemble into contiguous buffer.
    let mut out = vec![0u8; total_size];
    for (piece, maybe_data) in pieces.into_iter().enumerate() {
        let data = maybe_data.context("missing piece data")?;
        let expected = metadata_piece_size(piece as u32, total_size);
        let offset = piece * METADATA_PIECE_SIZE;
        let to_copy = expected
            .min(data.len())
            .min(out.len().saturating_sub(offset));
        out[offset..offset + to_copy].copy_from_slice(&data[..to_copy]);
    }

    Ok(out)
}

async fn wait_for_peer_handshake(conn: &mut PeerConnection) -> anyhow::Result<(u8, Option<u32>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timed out waiting for extension handshake");
        }
        let remaining = deadline - now;
        let msg = timeout(remaining, conn.receive()).await??;
        let Message::Extended { id, payload } = msg else {
            continue;
        };
        let ext = ExtensionMessage::decode(id, payload.as_ref())?;
        let ExtensionMessage::Handshake(peer_hs) = ext else {
            continue;
        };

        let Some(ut_id) = peer_hs.get_extension_id("ut_metadata") else {
            anyhow::bail!("peer did not advertise ut_metadata");
        };

        let total = peer_hs.metadata_size.and_then(|v| u32::try_from(v).ok());
        return Ok((ut_id, total));
    }
}

async fn request_piece(
    conn: &mut PeerConnection,
    ut_metadata_id: u8,
    piece: u32,
) -> anyhow::Result<()> {
    let payload = MetadataMessage::request(piece).encode()?;
    conn.send(Message::Extended {
        id: ut_metadata_id,
        payload,
    })
    .await?;
    Ok(())
}

async fn recv_metadata_msg(
    conn: &mut PeerConnection,
    ut_metadata_id: u8,
    timeout_dur: Duration,
) -> anyhow::Result<MetadataMessage> {
    // Peers may interleave many other messages (bitfield/have/choke/keep-alive)
    // while we are waiting for ut_metadata responses; keep reading until we
    // see the right extended message or time out.
    let deadline = tokio::time::Instant::now() + timeout_dur;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            anyhow::bail!("timed out waiting for ut_metadata message");
        }
        let remaining = deadline - now;
        let msg = timeout(remaining, conn.receive()).await??;
        let Message::Extended { id, payload } = msg else {
            continue;
        };
        if id != ut_metadata_id {
            continue;
        }
        return Ok(MetadataMessage::decode(payload.as_ref())?);
    }
}

fn parse_info_hash_hex(s: &str) -> anyhow::Result<[u8; 20]> {
    let bytes = hex::decode(s)?;
    if bytes.len() != 20 {
        anyhow::bail!("expected 20 bytes, got {}", bytes.len());
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn extract_name_from_info(info_bencode: &[u8]) -> anyhow::Result<String> {
    let v = bencode::decode(info_bencode)?;
    let name = v
        .get(b"name.utf-8")
        .and_then(|x| x.as_str())
        .or_else(|| v.get(b"name").and_then(|x| x.as_str()))
        .context("missing name")?;
    Ok(name.to_string())
}

async fn announce_seeders(
    tracker: &TrackerClient,
    info_hash: &InfoHash,
    peer_id: &[u8; 20],
    trackers: &[String],
) -> Option<i64> {
    let mut best: Option<i64> = None;
    for url in trackers {
        let params = AnnounceParams {
            url,
            info_hash,
            peer_id,
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: 1,
            event: TrackerEvent::Started,
        };

        let resp = timeout(Duration::from_secs(6), tracker.announce(params)).await;
        let Ok(Ok(resp)) = resp else {
            continue;
        };

        if let Some(complete) = resp.complete {
            let complete = complete as i64;
            best = Some(best.map_or(complete, |b| b.max(complete)));
        }
    }
    best
}
