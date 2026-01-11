mod enrich;
mod cleanup;
mod config;
mod index;
mod spider;
mod socks5;
mod storage;
mod web;

use anyhow::Context;
use std::path::PathBuf;

#[derive(Clone)]
pub struct AppState {
    pub config: config::Config,
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

    let config = config::Config::load()?;

    let data_dir = config.data_dir.clone();
    std::fs::create_dir_all(&data_dir).context("create data dir")?;

    let db = sled::open(data_dir.join("sled")).context("open sled db")?;
    // Build secondary indexes (one-time migration) so background tasks can find work without
    // scanning the full DB each loop.
    crate::storage::ensure_missing_info_index(&db).context("build missing-info index")?;
    crate::storage::ensure_cleanup_indexes(&db).context("build cleanup indexes")?;
    let index = index::SearchIndex::open_or_create(data_dir.join("tantivy"))
        .context("open/create tantivy index")?;

    let state = AppState {
        config: config.clone(),
        data_dir,
        db,
        index,
    };

    // Optional SOCKS5 proxy health-check (privacy).
    // This is best-effort and does not change behavior beyond logging.
    match crate::socks5::Socks5Config::from_env() {
        Some(Ok(cfg)) => match crate::socks5::Socks5UdpAssociate::connect(&cfg).await {
            Ok(sock) => {
                tracing::info!(proxy=%cfg.proxy, relay=%sock.relay_addr(), "socks5: udp associate OK");
            }
            Err(err) => {
                tracing::warn!(%err, proxy=%cfg.proxy, "socks5: udp associate failed (spider will disable; enrich DHT lookups will fail)");
            }
        },
        Some(Err(err)) => {
            tracing::warn!(%err, "socks5: invalid SERMA_SOCKS5_PROXY (spider will disable; enrich DHT lookups will fail)");
        }
        None => {}
    }

    // Background enrichment: DHT peer lookup -> ut_metadata info dict fetch -> persist full info -> reindex.
    tokio::spawn(enrich::run(state.clone()));

    // Autonomous discovery (DHT spider): harvest new hashes from DHT traffic.
    tokio::spawn(spider::run(state.clone()));

    // Periodic cleanup: remove inactive / low-seed torrents so they don't accumulate.
    tokio::spawn(cleanup::run(state.clone()));

    if let Some(addr) = config.http_addr {
        web::serve(state, addr).await
    } else {
        web::serve_dual_loopback(state, config.web_port).await
    }
}
