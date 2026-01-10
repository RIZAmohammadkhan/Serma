use crate::{AppState, storage};
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

const TORRENT_PREFIX: &[u8] = b"torrent:";
const META_TREE: &[u8] = b"meta";
const META_CLEANUP_CURSOR_V1: &[u8] = b"cleanup_cursor_v1";

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

pub async fn run(state: AppState) {
    // Allow disabling cleanup.
    if std::env::var("SERMA_CLEANUP")
        .ok()
        .is_some_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no"))
    {
        tracing::info!("cleanup: disabled via SERMA_CLEANUP");
        return;
    }

    // Defaults are chosen to be conservative with resources while still preventing buildup.
    // Cleanup does a bounded incremental scan; a longer period reduces steady-state overhead.
    let every_secs = env_u64("SERMA_CLEANUP_EVERY_SECS", 5 * 60);
    // Max number of records processed per tick.
    let batch = env_u64("SERMA_CLEANUP_BATCH", 200) as usize;
    // Wall-clock budget per tick (caps CPU usage even if decode/index deletes are expensive).
    let max_ms = env_u64("SERMA_CLEANUP_MAX_MS", 25);

    // Records not seen for this long are considered inactive.
    let ttl_secs = env_u64("SERMA_TORRENT_TTL_SECS", 24 * 60 * 60);

    // Give newly discovered hashes time to be enriched before pruning low-seed entries.
    let low_seed_grace_secs = env_u64("SERMA_LOW_SEED_GRACE_SECS", 20 * 60);

    let mut tick = interval(Duration::from_secs(every_secs.max(1)));

    loop {
        tick.tick().await;

        let meta = match state.db.open_tree(META_TREE) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(%err, "cleanup: failed opening meta tree");
                continue;
            }
        };

        let now = now_unix_ms();
        let ttl_ms = (ttl_secs as i64) * 1000;
        let grace_ms = (low_seed_grace_secs as i64) * 1000;

        let mut scanned: usize = 0;
        let mut deleted: usize = 0;

        let start = Instant::now();

        // Continue scanning from the last seen key so we don't re-scan the same prefix window
        // every tick. Cursor is persisted so restarts don't reset progress.
        let cursor = meta.get(META_CLEANUP_CURSOR_V1).ok().flatten().map(|v| v.to_vec());
        let start_bound = match cursor.as_deref() {
            Some(k) => Bound::Excluded(k.to_vec()),
            None => Bound::Included(TORRENT_PREFIX.to_vec()),
        };

        let mut last_key: Option<Vec<u8>> = None;

        for item in state
            .db
            .range((start_bound, Bound::Unbounded))
            .take(batch)
        {
            let (k, v) = match item {
                Ok(x) => x,
                Err(_) => continue,
            };

            // Stop when we leave the prefix range.
            if !k.starts_with(TORRENT_PREFIX) {
                break;
            }

            let record: storage::TorrentRecord = match storage::decode_torrent_record_maybe_migrate(&state.db, &k, &v) {
                Ok(r) => r,
                Err(_) => continue,
            };

            scanned += 1;
            last_key = Some(k.to_vec());

            let inactive = now.saturating_sub(record.last_seen_unix_ms) > ttl_ms;
            let low_seed_old = record.seeders < 2
                && now.saturating_sub(record.first_seen_unix_ms) > grace_ms;

            if inactive || low_seed_old {
                let _ = storage::delete(&state.db, &record.info_hash_hex);
                let _ = state.index.delete(&record.info_hash_hex);
                deleted += 1;
            }

            if scanned % 50 == 0 {
                tokio::task::yield_now().await;
            }

            if start.elapsed() >= Duration::from_millis(max_ms) {
                break;
            }
        }

        // Persist progress. If we hit the end of the prefix, reset cursor.
        if let Some(k) = last_key {
            let _ = meta.insert(META_CLEANUP_CURSOR_V1, k);
        } else {
            // Either DB is empty or we were already at the end of prefix range.
            // Reset to start so we sweep from the beginning on next tick.
            let _ = meta.remove(META_CLEANUP_CURSOR_V1);
        }

        if deleted > 0 {
            let _ = state.index.maybe_commit();
        }

        tracing::debug!(scanned, deleted, budget_ms = max_ms, "cleanup: sweep");
    }
}
