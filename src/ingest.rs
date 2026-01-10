use crate::AppState;
use crate::storage;
use tokio::io::{self, AsyncBufReadExt};

fn is_hex_40(s: &str) -> bool {
    s.len() == 40 && s.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

pub async fn run_file_or_stdin_ingest(state: AppState) {
    // If {SERMA_DATA_DIR}/hashes.txt exists, read it; otherwise read stdin.
    let path = state.data_dir.join("hashes.txt");

    let reader: Box<dyn tokio::io::AsyncBufRead + Unpin + Send> = if path.exists() {
        match tokio::fs::File::open(&path).await {
            Ok(file) => Box::new(tokio::io::BufReader::new(file)),
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "failed to open hashes file; falling back to stdin");
                Box::new(tokio::io::BufReader::new(io::stdin()))
            }
        }
    } else {
        Box::new(tokio::io::BufReader::new(io::stdin()))
    };

    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let candidate = line.trim().to_lowercase();
        if candidate.is_empty() {
            continue;
        }
        if !is_hex_40(&candidate) {
            tracing::debug!(value = %candidate, "skipping non-40-hex line");
            continue;
        }

        match storage::upsert_first_seen(&state.db, &candidate) {
            Ok(record) => {
                let title = record
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Torrent {}", &record.info_hash_hex));
                let magnet = record.magnet.clone().unwrap_or_default();

                if let Err(err) =
                    state
                        .index
                        .upsert(&record.info_hash_hex, &title, &magnet, record.seeders)
                {
                    tracing::warn!(%err, "failed to index record");
                }

                // Make small ingests visible without waiting for 100 documents.
                if let Err(err) = state.index.maybe_commit() {
                    tracing::debug!(%err, "tantivy commit skipped/failed");
                }

                tracing::info!(hash = %record.info_hash_hex, "ingested");
            }
            Err(err) => tracing::warn!(%err, "failed to upsert in sled"),
        }
    }
}
