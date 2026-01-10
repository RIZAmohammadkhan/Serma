use bincode::Options;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const TORRENT_RECORD_MAGIC: [u8; 4] = *b"SRM1";
const MISSING_INFO_TREE: &[u8] = b"idx_missing_info";
const LAST_SEEN_TREE: &[u8] = b"idx_last_seen";
const LOW_SEED_TREE: &[u8] = b"idx_low_seed";
const META_TREE: &[u8] = b"meta";
const META_MISSING_INFO_BUILT_V1: &[u8] = b"missing_info_index_built_v1";
const META_CLEANUP_INDEXES_BUILT_V1: &[u8] = b"cleanup_indexes_built_v1";

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

fn u64_be(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

fn ts_key(ts_unix_ms: i64, info_hash_hex: &str) -> Vec<u8> {
    // Sort by timestamp ascending, then by hash.
    let mut out = Vec::with_capacity(8 + info_hash_hex.len());
    let ts_u = (ts_unix_ms.max(0)) as u64;
    out.extend_from_slice(&u64_be(ts_u));
    out.extend_from_slice(info_hash_hex.as_bytes());
    out
}

fn parse_ts_key(key: &[u8]) -> Option<(i64, String)> {
    if key.len() < 8 {
        return None;
    }
    let mut ts_bytes = [0u8; 8];
    ts_bytes.copy_from_slice(&key[..8]);
    let ts = u64::from_be_bytes(ts_bytes) as i64;
    let hash = std::str::from_utf8(&key[8..]).ok()?.to_string();
    Some((ts, hash))
}

fn has_info(record: &TorrentRecord) -> bool {
    record
        .info_bencode_base64
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn missing_info_tree(db: &sled::Db) -> sled::Result<sled::Tree> {
    db.open_tree(MISSING_INFO_TREE)
}

fn last_seen_tree(db: &sled::Db) -> sled::Result<sled::Tree> {
    db.open_tree(LAST_SEEN_TREE)
}

fn low_seed_tree(db: &sled::Db) -> sled::Result<sled::Tree> {
    db.open_tree(LOW_SEED_TREE)
}

fn meta_tree(db: &sled::Db) -> sled::Result<sled::Tree> {
    db.open_tree(META_TREE)
}

fn sync_missing_info_index(db: &sled::Db, record: &TorrentRecord) -> anyhow::Result<()> {
    let tree = missing_info_tree(db)?;
    let key = record.info_hash_hex.as_bytes();
    if has_info(record) {
        let _ = tree.remove(key)?;
    } else {
        // Value is unused; presence of key indicates "needs enrich".
        tree.insert(key, &[])?;
    }
    Ok(())
}

fn sync_last_seen_index(db: &sled::Db, before: Option<&TorrentRecord>, after: &TorrentRecord) -> anyhow::Result<()> {
    let tree = last_seen_tree(db)?;

    if let Some(before) = before {
        if before.last_seen_unix_ms != after.last_seen_unix_ms {
            let _ = tree.remove(ts_key(before.last_seen_unix_ms, &before.info_hash_hex))?;
        }
    }

    tree.insert(ts_key(after.last_seen_unix_ms, &after.info_hash_hex), &[])?;
    Ok(())
}

fn sync_low_seed_index(db: &sled::Db, before: Option<&TorrentRecord>, after: &TorrentRecord) -> anyhow::Result<()> {
    let tree = low_seed_tree(db)?;
    let key = ts_key(after.first_seen_unix_ms, &after.info_hash_hex);

    let before_low = before.is_some_and(|r| r.seeders < 2);
    let after_low = after.seeders < 2;

    match (before_low, after_low) {
        (true, false) => {
            let _ = tree.remove(key)?;
        }
        (false, true) => {
            tree.insert(key, &[])?;
        }
        (true, true) => {
            // First-seen is immutable; no-op.
        }
        (false, false) => {}
    }
    Ok(())
}

pub fn cleanup_last_seen_tree(db: &sled::Db) -> anyhow::Result<sled::Tree> {
    Ok(last_seen_tree(db)?)
}

pub fn cleanup_low_seed_tree(db: &sled::Db) -> anyhow::Result<sled::Tree> {
    Ok(low_seed_tree(db)?)
}

pub fn end_key_for_ts(ts_unix_ms: i64) -> Vec<u8> {
    // Upper bound (inclusive) for all keys with timestamp <= ts_unix_ms.
    let mut out = Vec::with_capacity(8 + 1);
    let ts_u = (ts_unix_ms.max(0)) as u64;
    out.extend_from_slice(&u64_be(ts_u));
    out.push(0xFF);
    out
}

pub fn parse_cleanup_index_key(key: &[u8]) -> Option<(i64, String)> {
    parse_ts_key(key)
}

pub fn fix_last_seen_index_entry(
    db: &sled::Db,
    indexed_last_seen_unix_ms: i64,
    record: &TorrentRecord,
) -> anyhow::Result<()> {
    let tree = last_seen_tree(db)?;
    if indexed_last_seen_unix_ms != record.last_seen_unix_ms {
        let _ = tree.remove(ts_key(indexed_last_seen_unix_ms, &record.info_hash_hex))?;
        tree.insert(ts_key(record.last_seen_unix_ms, &record.info_hash_hex), &[])?;
    }
    Ok(())
}

pub fn fix_low_seed_index_entry(
    db: &sled::Db,
    indexed_first_seen_unix_ms: i64,
    record: &TorrentRecord,
) -> anyhow::Result<()> {
    let tree = low_seed_tree(db)?;
    if record.seeders >= 2 {
        let _ = tree.remove(ts_key(indexed_first_seen_unix_ms, &record.info_hash_hex))?;
        return Ok(());
    }

    if indexed_first_seen_unix_ms != record.first_seen_unix_ms {
        let _ = tree.remove(ts_key(indexed_first_seen_unix_ms, &record.info_hash_hex))?;
        tree.insert(ts_key(record.first_seen_unix_ms, &record.info_hash_hex), &[])?;
    }
    Ok(())
}

/// Ensures the missing-info index exists and is populated.
///
/// This replaces the previous runtime O(n) scan in `list_missing_info` with an indexed lookup.
/// Rebuilding can still be O(n) once, but happens only on first startup after upgrade.
pub fn ensure_missing_info_index(db: &sled::Db) -> anyhow::Result<()> {
    let meta = meta_tree(db)?;
    if meta.get(META_MISSING_INFO_BUILT_V1)?.is_some() {
        return Ok(());
    }

    let tree = missing_info_tree(db)?;
    let mut missing_count: usize = 0;
    let mut total: usize = 0;
    for item in db.scan_prefix(b"torrent:") {
        let (k, v) = item?;
        total += 1;
        let record = decode_torrent_record_maybe_migrate(db, &k, &v)?;
        if has_info(&record) {
            let _ = tree.remove(record.info_hash_hex.as_bytes())?;
        } else {
            tree.insert(record.info_hash_hex.as_bytes(), &[])?;
            missing_count += 1;
        }
    }

    meta.insert(META_MISSING_INFO_BUILT_V1, b"1")?;
    tracing::info!(total, missing_count, "storage: built missing-info index");
    Ok(())
}

/// Ensures cleanup indexes exist and are populated.
///
/// - `idx_last_seen`: ordered by `last_seen_unix_ms` for TTL pruning / eviction
/// - `idx_low_seed`: ordered by `first_seen_unix_ms` for pruning low-seed stale entries
///
/// This avoids periodic O(n) scans in the cleanup worker.
pub fn ensure_cleanup_indexes(db: &sled::Db) -> anyhow::Result<()> {
    let meta = meta_tree(db)?;
    if meta.get(META_CLEANUP_INDEXES_BUILT_V1)?.is_some() {
        return Ok(());
    }

    let last_seen = last_seen_tree(db)?;
    let low_seed = low_seed_tree(db)?;

    let mut total: usize = 0;
    let mut low_seed_count: usize = 0;
    for item in db.scan_prefix(b"torrent:") {
        let (k, v) = item?;
        total += 1;
        let record = decode_torrent_record_maybe_migrate(db, &k, &v)?;
        last_seen.insert(ts_key(record.last_seen_unix_ms, &record.info_hash_hex), &[])?;
        if record.seeders < 2 {
            low_seed.insert(ts_key(record.first_seen_unix_ms, &record.info_hash_hex), &[])?;
            low_seed_count += 1;
        }
    }

    meta.insert(META_CLEANUP_INDEXES_BUILT_V1, b"1")?;
    tracing::info!(total, low_seed_count, "storage: built cleanup indexes");
    Ok(())
}

pub fn upsert_first_seen(db: &sled::Db, info_hash_hex: &str) -> anyhow::Result<TorrentRecord> {
    let key = key_for_hash(info_hash_hex);
    let now = now_unix_ms();

    let existing = db.get(&key)?;

    let before = existing
        .as_ref()
        .and_then(|b| decode_torrent_record(b).ok())
        .map(|(r, _)| r);

    let record = if let Some(bytes) = existing.as_ref() {
        let mut record: TorrentRecord = decode_torrent_record(bytes)?.0;
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

    // Keep indexes consistent.
    db.insert(key, encode_torrent_record(&record)?)?;
    let _ = sync_missing_info_index(db, &record);
    let _ = sync_last_seen_index(db, before.as_ref(), &record);
    let _ = sync_low_seed_index(db, before.as_ref(), &record);
    Ok(record)
}

pub fn list_missing_info(db: &sled::Db, limit: usize) -> anyhow::Result<Vec<TorrentRecord>> {
    let tree = missing_info_tree(db)?;

    // If the index hasn't been built yet (e.g. user upgraded but restarted without
    // calling `ensure_missing_info_index`), we can still return quickly.
    // Startup should call `ensure_missing_info_index`; this is just a safe fallback.
    if tree.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for item in tree.iter() {
        let (hash_bytes, _) = item?;
        let hash_hex = std::str::from_utf8(&hash_bytes)
            .ok()
            .map(str::to_string);
        let Some(hash_hex) = hash_hex else {
            // Corrupt key; drop it.
            let _ = tree.remove(hash_bytes)?;
            continue;
        };

        let key = key_for_hash(&hash_hex);
        let Some(bytes) = db.get(&key)? else {
            // Record was deleted; drop index entry.
            let _ = tree.remove(hash_bytes)?;
            continue;
        };

        let record = decode_torrent_record_maybe_migrate(db, &key, &bytes)?;
        if has_info(&record) {
            // Index is stale; fix it.
            let _ = tree.remove(hash_bytes)?;
            continue;
        }

        out.push(record);
        if out.len() >= limit {
            break;
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
    let before = db
        .get(&key)?
        .and_then(|b| decode_torrent_record(&b).ok())
        .map(|(r, _)| r);
    db.insert(&key, encode_torrent_record(&record)?)?;
    let _ = sync_missing_info_index(db, &record);
    let _ = sync_last_seen_index(db, before.as_ref(), &record);
    let _ = sync_low_seed_index(db, before.as_ref(), &record);
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
    let before = db
        .get(&key)?
        .and_then(|b| decode_torrent_record(&b).ok())
        .map(|(r, _)| r);
    db.insert(&key, encode_torrent_record(&record)?)?;
    let _ = sync_missing_info_index(db, &record);
    let _ = sync_last_seen_index(db, before.as_ref(), &record);
    let _ = sync_low_seed_index(db, before.as_ref(), &record);
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
    let before = db
        .get(&key)?
        .and_then(|b| decode_torrent_record(&b).ok())
        .map(|(r, _)| r);
    db.insert(&key, encode_torrent_record(&record)?)?;
    let _ = sync_missing_info_index(db, &record);
    let _ = sync_last_seen_index(db, before.as_ref(), &record);
    let _ = sync_low_seed_index(db, before.as_ref(), &record);
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
    let before = db
        .get(&key)?
        .and_then(|b| decode_torrent_record(&b).ok())
        .map(|(r, _)| r);

    let _ = db.remove(&key)?;
    if let Ok(tree) = missing_info_tree(db) {
        let _ = tree.remove(info_hash_hex.as_bytes());
    }

    if let Some(before) = before.as_ref() {
        if let Ok(tree) = last_seen_tree(db) {
            let _ = tree.remove(ts_key(before.last_seen_unix_ms, &before.info_hash_hex));
        }
        if let Ok(tree) = low_seed_tree(db) {
            let _ = tree.remove(ts_key(before.first_seen_unix_ms, &before.info_hash_hex));
        }
    }
    Ok(())
}
