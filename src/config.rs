use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,

    // Web
    pub http_addr: Option<SocketAddr>,
    pub web_port: u16,

    // Spider
    pub spider_enabled: bool,
    pub spider_bind: String,
    pub spider_bootstrap: Vec<String>,
    pub spider_max_known_nodes: usize,
    pub spider_seen_rotate_every_secs: u64,
    pub spider_seen_bits_pow2: u32,
    pub spider_seen_k: u8,
    pub spider_sample_every_secs: u64,
    pub spider_sample_per_tick: usize,
    pub spider_max_samples_per_msg: usize,
    pub spider_bootstrap_every_secs: u64,
    pub spider_gc_every_secs: u64,

    // Enrich
    pub enrich_missing_scan_limit: usize,
    pub enrich_max_concurrent: usize,
    pub enrich_peers_per_hash: usize,
    pub enrich_dht_bootstrap: Vec<String>,
    pub enrich_dht_query_timeout_ms: u64,
    pub enrich_dht_max_queries_per_hash: usize,
    pub enrich_dht_get_peers_timeout_secs: u64,
    pub enrich_dht_overall_deadline_secs: u64,
    pub enrich_dht_inflight: usize,
    pub enrich_dht_recv_timeout_ms: u64,
    pub enrich_metadata_inflight: usize,
    pub enrich_metadata_overall_timeout_secs: u64,

    // Cleanup
    pub cleanup_enabled: bool,
    pub cleanup_every_secs: u64,
    pub cleanup_batch: usize,
    pub cleanup_max_ms: u64,
    pub torrent_ttl_secs: u64,
    pub low_seed_grace_secs: u64,
    pub max_torrents: usize,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        // If a .env file exists, load it. If not, keep going.
        // Precedence: process env > .env > code defaults.
        let _ = dotenvy::dotenv();
        Self::from_env()
    }

    fn from_env() -> anyhow::Result<Self> {
        let data_dir = env_pathbuf("SERMA_DATA_DIR", "data");

        let http_addr = env_opt_string("SERMA_ADDR")
            .map(|s| SocketAddr::from_str(&s).map_err(|e| anyhow::anyhow!("parse SERMA_ADDR: {e}")))
            .transpose()?;

        let web_port = env_u16("SERMA_WEB_PORT", 3000);

        let spider_enabled = env_enabled("SERMA_SPIDER", true);
        let spider_bind = env_string("SERMA_SPIDER_BIND", "0.0.0.0:0");
        let spider_bootstrap = env_csv_strings(
            "SERMA_SPIDER_BOOTSTRAP",
            &[
                "router.bittorrent.com:6881",
                "dht.transmissionbt.com:6881",
                "router.utorrent.com:6881",
            ],
        );
        let spider_max_known_nodes = env_usize("SERMA_SPIDER_MAX_KNOWN_NODES", 10_000);
        let spider_seen_rotate_every_secs = env_u64("SERMA_SPIDER_SEEN_ROTATE_EVERY_SECS", 15 * 60);
        let spider_seen_bits_pow2 = env_u32("SERMA_SPIDER_SEEN_BITS_POW2", 26);
        let spider_seen_k = env_u8("SERMA_SPIDER_SEEN_K", 12);
        let spider_sample_every_secs = env_u64("SERMA_SPIDER_SAMPLE_EVERY_SECS", 5);
        let spider_sample_per_tick = env_usize("SERMA_SPIDER_SAMPLE_PER_TICK", 12);
        let spider_max_samples_per_msg = env_usize("SERMA_SPIDER_MAX_SAMPLES_PER_MSG", 256);
        let spider_bootstrap_every_secs = env_u64("SERMA_SPIDER_BOOTSTRAP_EVERY_SECS", 15);
        let spider_gc_every_secs = env_u64("SERMA_SPIDER_GC_EVERY_SECS", 30);

        let enrich_missing_scan_limit = env_usize("SERMA_ENRICH_MISSING_SCAN_LIMIT", 200);
        let enrich_max_concurrent = env_usize("SERMA_ENRICH_MAX_CONCURRENT", 64);
        let enrich_peers_per_hash = env_usize("SERMA_ENRICH_PEERS_PER_HASH", 64);
        let enrich_dht_bootstrap = env_csv_strings(
            "SERMA_ENRICH_DHT_BOOTSTRAP",
            &[
                "router.bittorrent.com:6881",
                "dht.transmissionbt.com:6881",
                "router.utorrent.com:6881",
            ],
        );
        let enrich_dht_query_timeout_ms = env_u64("SERMA_ENRICH_DHT_QUERY_TIMEOUT_MS", 900);
        let enrich_dht_max_queries_per_hash = env_usize("SERMA_ENRICH_DHT_MAX_QUERIES_PER_HASH", 32);
        let enrich_dht_get_peers_timeout_secs = env_u64("SERMA_ENRICH_DHT_GET_PEERS_TIMEOUT_SECS", 12);
        let enrich_dht_overall_deadline_secs = env_u64("SERMA_ENRICH_DHT_OVERALL_DEADLINE_SECS", 10);
        let enrich_dht_inflight = env_usize("SERMA_ENRICH_DHT_INFLIGHT", 8);
        let enrich_dht_recv_timeout_ms = env_u64("SERMA_ENRICH_DHT_RECV_TIMEOUT_MS", 250);
        let enrich_metadata_inflight = env_usize("SERMA_ENRICH_METADATA_INFLIGHT", 8);
        let enrich_metadata_overall_timeout_secs =
            env_u64("SERMA_ENRICH_METADATA_OVERALL_TIMEOUT_SECS", 16);

        let cleanup_enabled = env_enabled("SERMA_CLEANUP", true);
        let cleanup_every_secs = env_u64("SERMA_CLEANUP_EVERY_SECS", 10);
        let cleanup_batch = env_usize("SERMA_CLEANUP_BATCH", 5_000);
        let cleanup_max_ms = env_u64("SERMA_CLEANUP_MAX_MS", 1_000);
        let torrent_ttl_secs = env_u64("SERMA_TORRENT_TTL_SECS", 24 * 60 * 60);
        let low_seed_grace_secs = env_u64("SERMA_LOW_SEED_GRACE_SECS", 20 * 60);
        let max_torrents = env_usize("SERMA_MAX_TORRENTS", 0);

        Ok(Self {
            data_dir,
            http_addr,
            web_port,

            spider_enabled,
            spider_bind,
            spider_bootstrap,
            spider_max_known_nodes,
            spider_seen_rotate_every_secs,
            spider_seen_bits_pow2,
            spider_seen_k,
            spider_sample_every_secs,
            spider_sample_per_tick,
            spider_max_samples_per_msg,
            spider_bootstrap_every_secs,
            spider_gc_every_secs,

            enrich_missing_scan_limit,
            enrich_max_concurrent,
            enrich_peers_per_hash,
            enrich_dht_bootstrap,
            enrich_dht_query_timeout_ms,
            enrich_dht_max_queries_per_hash,
            enrich_dht_get_peers_timeout_secs,
            enrich_dht_overall_deadline_secs,
            enrich_dht_inflight,
            enrich_dht_recv_timeout_ms,
            enrich_metadata_inflight,
            enrich_metadata_overall_timeout_secs,

            cleanup_enabled,
            cleanup_every_secs,
            cleanup_batch,
            cleanup_max_ms,
            torrent_ttl_secs,
            low_seed_grace_secs,
            max_torrents,
        })
    }
}

fn env_opt_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_string(name: &str, default: &str) -> String {
    env_opt_string(name).unwrap_or_else(|| default.to_string())
}

fn env_pathbuf(name: &str, default: &str) -> PathBuf {
    PathBuf::from(env_string(name, default))
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(default)
}

fn env_u16(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(default)
}

fn env_u8(name: &str, default: u8) -> u8 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u8>().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_csv_strings(name: &str, defaults: &[&str]) -> Vec<String> {
    if let Some(s) = env_opt_string(name) {
        let v: Vec<String> = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    defaults.iter().map(|s| s.to_string()).collect()
}

fn env_enabled(name: &str, default: bool) -> bool {
    match env_opt_string(name) {
        None => default,
        Some(v) => {
            let v = v.to_ascii_lowercase();
            if matches!(v.as_str(), "0" | "false" | "off" | "no") {
                return false;
            }
            if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                return true;
            }
            default
        }
    }
}
