use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
        let mut record: TorrentRecord = serde_json::from_slice(&bytes)?;
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

    db.insert(key, serde_json::to_vec(&record)?)?;
    Ok(record)
}

pub fn list_missing_info(db: &sled::Db, limit: usize) -> anyhow::Result<Vec<TorrentRecord>> {
    let mut out = Vec::new();
    for item in db.scan_prefix(b"torrent:") {
        let (_k, v) = item?;
        let record: TorrentRecord = serde_json::from_slice(&v)?;
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
    db.insert(key, serde_json::to_vec(&record)?)?;
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
    db.insert(key, serde_json::to_vec(&record)?)?;
    Ok(record)
}

pub fn get(db: &sled::Db, info_hash_hex: &str) -> anyhow::Result<Option<TorrentRecord>> {
    let key = key_for_hash(info_hash_hex);
    let Some(bytes) = db.get(key)? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}
