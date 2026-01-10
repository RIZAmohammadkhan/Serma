use bincode::Options;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const TORRENT_RECORD_MAGIC: [u8; 4] = *b"SRM1";

fn bincode_opts() -> impl bincode::Options {
    // Varint encoding reduces disk usage for small integers.
    // Limit prevents accidental OOM / huge allocations on corrupted data.
    bincode::DefaultOptions::new()
        .with_varint_encoding()
        .with_limit(16 * 1024 * 1024)
}

fn encode_torrent_record(record: &TorrentRecord) -> anyhow::Result<Vec<u8>> {
    let payload = bincode_opts().serialize(record)?;
    let mut out = Vec::with_capacity(TORRENT_RECORD_MAGIC.len() + payload.len());
    out.extend_from_slice(&TORRENT_RECORD_MAGIC);
    out.extend_from_slice(&payload);
    Ok(out)
}

fn decode_torrent_record(bytes: &[u8]) -> anyhow::Result<(TorrentRecord, bool)> {
    if bytes.starts_with(&TORRENT_RECORD_MAGIC) {
        let payload = &bytes[TORRENT_RECORD_MAGIC.len()..];
        let record: TorrentRecord = bincode_opts().deserialize(payload)?;
        Ok((record, false))
    } else {
        // Backward-compat: legacy JSON values.
        let record: TorrentRecord = serde_json::from_slice(bytes)?;
        Ok((record, true))
    }
}

pub fn decode_torrent_record_maybe_migrate(
    db: &sled::Db,
    key: &[u8],
    bytes: &[u8],
) -> anyhow::Result<TorrentRecord> {
    let (record, was_json) = decode_torrent_record(bytes)?;
    if was_json {
        match encode_torrent_record(&record) {
            Ok(new_bytes) => {
                if let Err(e) = db.insert(key, new_bytes) {
                    tracing::warn!(error = %e, "failed to migrate torrent record to binary");
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to encode torrent record during migration"),
        }
    }
    Ok(record)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentRecord {
    pub info_hash_hex: String,
    pub title: Option<String>,
    pub magnet: Option<String>,
    pub seeders: i64,
    #[serde(default)]
    pub info_bencode_base64: Option<String>,
    pub first_seen_unix_ms: i64,
    pub last_seen_unix_ms: i64,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn key_for_hash(info_hash_hex: &str) -> Vec<u8> {
    let mut key = b"torrent:".to_vec();
    key.extend_from_slice(info_hash_hex.as_bytes());
    key
}

pub fn upsert_first_seen(db: &sled::Db, info_hash_hex: &str) -> anyhow::Result<TorrentRecord> {
    let key = key_for_hash(info_hash_hex);
    let now = now_unix_ms();

    let existing = db.get(&key)?;
    let record = if let Some(bytes) = existing {
        let mut record: TorrentRecord = decode_torrent_record(&bytes)?.0;
        record.last_seen_unix_ms = now;
        record
    } else {
        TorrentRecord {
            info_hash_hex: info_hash_hex.to_string(),
            title: None,
            magnet: None,
            seeders: 0,
            info_bencode_base64: None,
            first_seen_unix_ms: now,
            last_seen_unix_ms: now,
        }
    };

    db.insert(key, encode_torrent_record(&record)?)?;
    Ok(record)
}

pub fn list_missing_info(db: &sled::Db, limit: usize) -> anyhow::Result<Vec<TorrentRecord>> {
    let mut out = Vec::new();
    for item in db.scan_prefix(b"torrent:") {
        let (k, v) = item?;
        let record = decode_torrent_record_maybe_migrate(db, &k, &v)?;
        let has_info = record
            .info_bencode_base64
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        if !has_info {
            out.push(record);
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

pub fn set_metadata(
    db: &sled::Db,
    info_hash_hex: &str,
    title: Option<&str>,
    info_bencode_base64: &str,
) -> anyhow::Result<TorrentRecord> {
    let mut record = upsert_first_seen(db, info_hash_hex)?;
    if let Some(title) = title {
        if !title.trim().is_empty() {
            record.title = Some(title.to_string());
        }
    }
    record.info_bencode_base64 = Some(info_bencode_base64.to_string());
    let key = key_for_hash(info_hash_hex);
    db.insert(key, encode_torrent_record(&record)?)?;
    Ok(record)
}

pub fn set_seeders(
    db: &sled::Db,
    info_hash_hex: &str,
    seeders: i64,
) -> anyhow::Result<TorrentRecord> {
    let mut record = upsert_first_seen(db, info_hash_hex)?;
    record.seeders = seeders;
    let key = key_for_hash(info_hash_hex);
    db.insert(key, encode_torrent_record(&record)?)?;
    Ok(record)
}

pub fn set_magnet(
    db: &sled::Db,
    info_hash_hex: &str,
    magnet: &str,
) -> anyhow::Result<TorrentRecord> {
    let mut record = upsert_first_seen(db, info_hash_hex)?;
    if !magnet.trim().is_empty() {
        record.magnet = Some(magnet.to_string());
    }
    let key = key_for_hash(info_hash_hex);
    db.insert(key, encode_torrent_record(&record)?)?;
    Ok(record)
}

pub fn get(db: &sled::Db, info_hash_hex: &str) -> anyhow::Result<Option<TorrentRecord>> {
    let key = key_for_hash(info_hash_hex);
    let Some(bytes) = db.get(&key)? else {
        return Ok(None);
    };

    Ok(Some(decode_torrent_record_maybe_migrate(db, &key, &bytes)?))
}

pub fn delete(db: &sled::Db, info_hash_hex: &str) -> anyhow::Result<()> {
    let key = key_for_hash(info_hash_hex);
    let _ = db.remove(key)?;
    Ok(())
}
