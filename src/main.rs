mod enrich;
mod index;
mod ingest;
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
    let index = index::SearchIndex::open_or_create(data_dir.join("tantivy"))
        .context("open/create tantivy index")?;

    let state = AppState {
        data_dir,
        db,
        index,
    };

    // MVP ingestion: reads one info_hash (40 hex chars) per line from a file
    // or from stdin if no file is provided.
    tokio::spawn(ingest::run_file_or_stdin_ingest(state.clone()));

    // Background enrichment: DHT peer lookup -> ut_metadata info dict fetch -> persist full info -> reindex.
    tokio::spawn(enrich::run(state.clone()));

    // Autonomous discovery (DHT spider): harvest new hashes from DHT traffic.
    tokio::spawn(spider::run(state.clone()));

    let addr = std::env::var("SERMA_ADDR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1:3000".to_string());
    let addr = std::net::SocketAddr::from_str(&addr).context("parse SERMA_ADDR")?;

    web::serve(state, addr).await
}
