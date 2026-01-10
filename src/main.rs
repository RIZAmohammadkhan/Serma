mod enrich;
mod cleanup;
mod index;
mod spider;
mod storage;
mod web;

use anyhow::Context;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone)]
pub struct AppState {
    pub data_dir: PathBuf,
    pub db: sled::Db,
    pub index: index::SearchIndex,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = std::env::var("SERMA_DATA_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    std::fs::create_dir_all(&data_dir).context("create data dir")?;

    let db = sled::open(data_dir.join("sled")).context("open sled db")?;
    // Build secondary indexes (one-time migration) so background tasks can find work without
    // scanning the full DB each loop.
    crate::storage::ensure_missing_info_index(&db).context("build missing-info index")?;
    crate::storage::ensure_cleanup_indexes(&db).context("build cleanup indexes")?;
    let index = index::SearchIndex::open_or_create(data_dir.join("tantivy"))
        .context("open/create tantivy index")?;

    let state = AppState {
        data_dir,
        db,
        index,
    };

    // Background enrichment: DHT peer lookup -> ut_metadata info dict fetch -> persist full info -> reindex.
    tokio::spawn(enrich::run(state.clone()));

    // Autonomous discovery (DHT spider): harvest new hashes from DHT traffic.
    tokio::spawn(spider::run(state.clone()));

    // Periodic cleanup: remove inactive / low-seed torrents so they don't accumulate.
    tokio::spawn(cleanup::run(state.clone()));

    let addr = std::env::var("SERMA_ADDR")
        .ok()
        .filter(|s| !s.trim().is_empty());

    if let Some(addr) = addr {
        let addr = std::net::SocketAddr::from_str(&addr).context("parse SERMA_ADDR")?;
        web::serve(state, addr).await
    } else {
        web::serve_dual_loopback(state, 3000).await
    }
}
