use crate::{AppState, storage};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

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
    let every_secs = env_u64("SERMA_CLEANUP_EVERY_SECS", 60);
    let batch = env_u64("SERMA_CLEANUP_BATCH", 500) as usize;

    // Records not seen for this long are considered inactive.
    let ttl_secs = env_u64("SERMA_TORRENT_TTL_SECS", 24 * 60 * 60);

    // Give newly discovered hashes time to be enriched before pruning low-seed entries.
    let low_seed_grace_secs = env_u64("SERMA_LOW_SEED_GRACE_SECS", 20 * 60);

    let mut tick = interval(Duration::from_secs(every_secs.max(1)));

    loop {
        tick.tick().await;

        let now = now_unix_ms();
        let ttl_ms = (ttl_secs as i64) * 1000;
        let grace_ms = (low_seed_grace_secs as i64) * 1000;

        let mut scanned: usize = 0;
        let mut deleted: usize = 0;

        for item in state.db.scan_prefix(b"torrent:").take(batch) {
            let (k, v) = match item {
                Ok(x) => x,
                Err(_) => continue,
            };

            let record: storage::TorrentRecord = match storage::decode_torrent_record_maybe_migrate(&state.db, &k, &v) {
                Ok(r) => r,
                Err(_) => continue,
            };

            scanned += 1;

            let inactive = now.saturating_sub(record.last_seen_unix_ms) > ttl_ms;
            let low_seed_old = record.seeders < 2
                && now.saturating_sub(record.first_seen_unix_ms) > grace_ms;

            if inactive || low_seed_old {
                let _ = storage::delete(&state.db, &record.info_hash_hex);
                let _ = state.index.delete(&record.info_hash_hex);
                deleted += 1;
            }
        }

        if deleted > 0 {
            let _ = state.index.maybe_commit();
        }

        tracing::debug!(scanned, deleted, "cleanup: sweep");
    }
}
