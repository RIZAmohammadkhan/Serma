use crate::{AppState, storage};
use std::ops::Bound;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::interval;

const TORRENT_PREFIX: &[u8] = b"torrent:";

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

    // Cleanup is index-driven, so running more frequently is cheap.
    // Defaults are tuned to prevent unbounded growth without monopolizing CPU.
    let every_secs = env_u64("SERMA_CLEANUP_EVERY_SECS", 10);
    // Max number of index entries processed per tick.
    let batch = env_u64("SERMA_CLEANUP_BATCH", 5_000) as usize;
    // Wall-clock budget per tick.
    let max_ms = env_u64("SERMA_CLEANUP_MAX_MS", 1_000);

    // Records not seen for this long are considered inactive.
    let ttl_secs = env_u64("SERMA_TORRENT_TTL_SECS", 24 * 60 * 60);

    // Give newly discovered hashes time to be enriched before pruning low-seed entries.
    let low_seed_grace_secs = env_u64("SERMA_LOW_SEED_GRACE_SECS", 20 * 60);

    // Optional hard cap to prevent disk growth even if ingestion rate is extremely high.
    // If set (> 0), we evict oldest-by-last_seen until we're under the limit.
    let max_records = env_u64("SERMA_MAX_TORRENTS", 0) as usize;

    let mut tick = interval(Duration::from_secs(every_secs.max(1)));

    loop {
        tick.tick().await;

        let last_seen = match storage::cleanup_last_seen_tree(&state.db) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(%err, "cleanup: failed opening last_seen index");
                continue;
            }
        };

        let low_seed = match storage::cleanup_low_seed_tree(&state.db) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(%err, "cleanup: failed opening low_seed index");
                continue;
            }
        };

        let now = now_unix_ms();
        let ttl_ms = (ttl_secs as i64) * 1000;
        let grace_ms = (low_seed_grace_secs as i64) * 1000;

        let cutoff_last_seen = now.saturating_sub(ttl_ms);
        let cutoff_first_seen = now.saturating_sub(grace_ms);

        let mut scanned: usize = 0;
        let mut deleted: usize = 0;
        let mut stale_fixed: usize = 0;

        let start = Instant::now();

        // Phase 1: TTL cleanup driven by last_seen index.
        // Range keys are [last_seen_be][hash]. We scan up to cutoff_last_seen.
        let end_key = storage::end_key_for_ts(cutoff_last_seen);
        for item in last_seen
            .range((Bound::Unbounded, Bound::Included(end_key)))
            .take(batch)
        {
            let (idx_key, _) = match item {
                Ok(x) => x,
                Err(_) => continue,
            };

            let Some((indexed_last_seen, hash_hex)) = storage::parse_cleanup_index_key(&idx_key) else {
                let _ = last_seen.remove(idx_key);
                continue;
            };

            scanned += 1;

            let mut db_key = TORRENT_PREFIX.to_vec();
            db_key.extend_from_slice(hash_hex.as_bytes());
            let Some(bytes) = state.db.get(&db_key).ok().flatten() else {
                // Record is gone; drop stale index entry.
                let _ = last_seen.remove(idx_key);
                continue;
            };

            let record = match storage::decode_torrent_record_maybe_migrate(&state.db, &db_key, &bytes) {
                Ok(r) => r,
                Err(_) => continue,
            };

            if record.last_seen_unix_ms <= cutoff_last_seen {
                let _ = storage::delete(&state.db, &record.info_hash_hex);
                let _ = state.index.delete(&record.info_hash_hex);
                deleted += 1;
            } else {
                // Index entry is stale; fix it so we don't keep revisiting.
                if storage::fix_last_seen_index_entry(&state.db, indexed_last_seen, &record).is_ok() {
                    stale_fixed += 1;
                }
            }

            if scanned % 250 == 0 {
                tokio::task::yield_now().await;
            }

            if start.elapsed() >= Duration::from_millis(max_ms) {
                break;
            }
        }

        // Phase 2: low-seed cleanup driven by first_seen index.
        // We scan low-seed candidates older than grace.
        if start.elapsed() < Duration::from_millis(max_ms) {
            let remaining = Duration::from_millis(max_ms).saturating_sub(start.elapsed());
            let end_key = storage::end_key_for_ts(cutoff_first_seen);
            for item in low_seed
                .range((Bound::Unbounded, Bound::Included(end_key)))
                .take(batch)
            {
                let (idx_key, _) = match item {
                    Ok(x) => x,
                    Err(_) => continue,
                };

                let Some((indexed_first_seen, hash_hex)) = storage::parse_cleanup_index_key(&idx_key) else {
                    let _ = low_seed.remove(idx_key);
                    continue;
                };

                scanned += 1;

                let mut db_key = TORRENT_PREFIX.to_vec();
                db_key.extend_from_slice(hash_hex.as_bytes());
                let Some(bytes) = state.db.get(&db_key).ok().flatten() else {
                    let _ = low_seed.remove(idx_key);
                    continue;
                };

                let record = match storage::decode_torrent_record_maybe_migrate(&state.db, &db_key, &bytes) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                if record.seeders >= 2 {
                    // No longer low-seed; index is stale.
                    if storage::fix_low_seed_index_entry(&state.db, indexed_first_seen, &record).is_ok() {
                        stale_fixed += 1;
                    }
                } else {
                    let old_enough = now.saturating_sub(record.first_seen_unix_ms) > grace_ms;
                    if old_enough {
                        let _ = storage::delete(&state.db, &record.info_hash_hex);
                        let _ = state.index.delete(&record.info_hash_hex);
                        deleted += 1;
                    } else {
                        // Still in grace; ensure key is consistent.
                        if storage::fix_low_seed_index_entry(&state.db, indexed_first_seen, &record).is_ok() {
                            stale_fixed += 1;
                        }
                    }
                }

                if scanned % 250 == 0 {
                    tokio::task::yield_now().await;
                }

                if start.elapsed() >= Duration::from_millis(max_ms) {
                    break;
                }
                if remaining == Duration::ZERO {
                    break;
                }
            }
        }

        // Phase 3 (optional): enforce max-record cap by evicting oldest by last_seen.
        // This prevents unbounded growth even if TTL is long and ingestion is massive.
        if max_records > 0 {
            // Safety: we only do eviction if we still have budget.
            while start.elapsed() < Duration::from_millis(max_ms) {
                let len = last_seen.len();
                if len <= max_records {
                    break;
                }

                // Evict one oldest record per loop iteration.
                let mut evicted_one = false;
                for item in last_seen.iter().take(1) {
                    let (idx_key, _) = match item {
                        Ok(x) => x,
                        Err(_) => break,
                    };
                    let Some((_indexed_last_seen, hash_hex)) = storage::parse_cleanup_index_key(&idx_key) else {
                        let _ = last_seen.remove(idx_key);
                        break;
                    };

                    // Double-check record exists.
                    let mut db_key = TORRENT_PREFIX.to_vec();
                    db_key.extend_from_slice(hash_hex.as_bytes());
                    if let Some(bytes) = state.db.get(&db_key).ok().flatten() {
                        if let Ok(record) = storage::decode_torrent_record_maybe_migrate(&state.db, &db_key, &bytes) {
                            let _ = storage::delete(&state.db, &record.info_hash_hex);
                            let _ = state.index.delete(&record.info_hash_hex);
                            deleted += 1;
                            evicted_one = true;
                        }
                    } else {
                        let _ = last_seen.remove(idx_key);
                        evicted_one = true;
                    }
                }

                if !evicted_one {
                    break;
                }
            }
        }

        if deleted > 0 {
            let _ = state.index.maybe_commit();
        }

        tracing::debug!(scanned, deleted, stale_fixed, budget_ms = max_ms, cutoff_last_seen, cutoff_first_seen, max_records, "cleanup: sweep");
    }
}
