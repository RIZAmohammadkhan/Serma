use crate::{AppState, storage};
use anyhow::Context;
use base64::Engine as _;
use bytes::Bytes;
use rbit::bencode;
use rbit::dht::DhtServer;
use rbit::metainfo::{InfoHash, MagnetLink};
use rbit::peer::{
    ExtensionHandshake, ExtensionMessage, METADATA_PIECE_SIZE, Message, MetadataMessage,
    MetadataMessageType, PeerConnection, PeerId, metadata_piece_size,
};
use rbit::tracker::{AnnounceParams, TrackerClient, TrackerEvent};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, timeout};

const MISSING_SCAN_LIMIT: usize = 64;
const MAX_CONCURRENT_ENRICH: usize = 8;
const PEERS_PER_HASH: usize = 12;

pub async fn run(state: AppState) {
    let dht = match DhtServer::bind(0).await {
        Ok(dht) => dht,
        Err(err) => {
            tracing::warn!(%err, "enrich: failed to bind DHT server; enrichment disabled");
            return;
        }
    };

    if let Err(err) = dht.bootstrap().await {
        tracing::warn!(%err, "enrich: DHT bootstrap failed (continuing anyway)");
    }

    let dht = Arc::new(dht);
    {
        let dht_bg = dht.clone();
        tokio::spawn(async move {
            if let Err(err) = dht_bg.run().await {
                tracing::warn!(%err, "enrich: DHT run loop exited");
            }
        });
    }

    let tracker = Arc::new(TrackerClient::new());
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_ENRICH));

    loop {
        let missing = match storage::list_missing_info(&state.db, MISSING_SCAN_LIMIT) {
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
            let dht = dht.clone();
            let tracker = tracker.clone();

            tokio::spawn(async move {
                let _permit = permit;
                if let Err(err) = enrich_one(&state, &dht, &tracker, record).await {
                    tracing::debug!(%err, "enrich: failed");
                }
            });
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn enrich_one(
    state: &AppState,
    dht: &DhtServer,
    tracker: &TrackerClient,
    record: storage::TorrentRecord,
) -> anyhow::Result<()> {
    let info_hash_bytes = parse_info_hash_hex(&record.info_hash_hex)
        .with_context(|| format!("invalid info hash: {}", record.info_hash_hex))?;

    let peers = timeout(Duration::from_secs(12), dht.get_peers(info_hash_bytes))
        .await
        .context("dht get_peers timed out")??;

    if peers.is_empty() {
        return Ok(());
    }

    // Best-effort: use DHT peer count as a lower-bound popularity signal.
    // (Trackers provide real seeder counts when available.)
    if peers.len() as i64 > record.seeders {
        let _ = storage::set_seeders(&state.db, &record.info_hash_hex, peers.len() as i64);
    }

    let mut metadata: Option<Vec<u8>> = None;
    for peer in peers.into_iter().take(PEERS_PER_HASH) {
        match fetch_ut_metadata(peer, info_hash_bytes).await {
            Ok(info_bytes) => {
                metadata = Some(info_bytes);
                break;
            }
            Err(err) => {
                tracing::trace!(%err, addr = %peer, "enrich: peer failed");
            }
        }
    }

    let Some(info_bytes) = metadata else {
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
    let _ = state.index.upsert(
        &updated.info_hash_hex,
        &title_for_index,
        &magnet_for_index,
        updated.seeders,
    );
    let _ = state.index.maybe_commit();
    Ok(())
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
    let msg = timeout(timeout_dur, conn.receive()).await??;
    let Message::Extended { id, payload } = msg else {
        anyhow::bail!("unexpected non-extended message");
    };
    if id != ut_metadata_id {
        anyhow::bail!("unexpected extension id: {id}");
    }
    Ok(MetadataMessage::decode(payload.as_ref())?)
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
