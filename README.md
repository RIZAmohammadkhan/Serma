# Serma

![Logo](logo.png)

**The local index.** A self-hosted BitTorrent DHT spider and search engine.

Serma autonomously discovers torrents from the BitTorrent DHT network, enriches metadata, and provides a clean web interface for searching your personal torrent index.

## Features

- 🕷️ **Autonomous DHT Spider**: Crawls the BitTorrent DHT network to discover new torrents
- 🔍 **Full-Text Search**: Fast search powered by Tantivy (Rust's Lucene alternative)
- 📊 **Metadata Enrichment**: Automatically fetches torrent metadata using the ut_metadata extension
- 🧹 **Automatic Cleanup**: Removes inactive/low-seed torrents to keep the index fresh
- 🌐 **Clean Web UI**: Minimalist dark-mode interface for browsing and searching
- 🚀 **High Performance**: Built in Rust for speed and efficiency
- 💾 **Embedded Storage**: Uses Sled (embedded database) and Tantivy (search index)

## Architecture

Serma consists of several background tasks:

- **Spider** (`spider.rs`): BEP-5 DHT crawler that discovers info hashes from DHT traffic
- **Enricher** (`enrich.rs`): Fetches full torrent metadata via DHT peer lookup and ut_metadata protocol
- **Indexer** (`index.rs`): Maintains a full-text search index using Tantivy
- **Cleanup** (`cleanup.rs`): Periodic task to remove stale/low-quality torrents
- **Web Server** (`web.rs`): Axum-based HTTP server providing search UI and API

## Requirements

- **Rust** 1.75 or later (edition 2024)
- **Linux, macOS, or Windows** (tested on Linux)
- ~16-32 GB disk space for a meaningful index (grows over time)
- Open UDP port (optional, but recommended for better DHT connectivity)

## Quick Start

### 1. Clone and Build

```bash
git clone <repository-url> serma
cd serma
cargo build --release
```

### 2. Run

```bash
./target/release/serma
```

By default, Serma:
- Stores data in `./data` directory
- Serves web UI on `http://localhost:3000`
- Uses an ephemeral UDP port for DHT traffic

### 3. Open the Web Interface

Navigate to `http://localhost:3000` in your browser to start searching.

## Configuration

Serma is configured via environment variables, optionally loaded from a local `.env` file.

### Using an `.env` file (recommended)

```bash
cp .env.example .env
$EDITOR .env
./target/release/serma
```

Precedence:
- Process environment variables override `.env`
- `.env` overrides built-in defaults

The complete, up-to-date list of configuration options lives in `.env.example`.

### Common variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SERMA_DATA_DIR` | `data` | Directory for database and index storage |
| `SERMA_ADDR` | (unset) | HTTP server bind address (if unset, dual loopback is used) |
| `SERMA_WEB_PORT` | `3000` | Web port used when `SERMA_ADDR` is unset (binds `127.0.0.1` and `::1`) |
| `SERMA_SPIDER` | enabled | Set to `0`, `false`, `off`, or `no` to disable DHT spider |
| `SERMA_SPIDER_BIND` | `0.0.0.0:0` | UDP bind address for DHT spider |
| `SERMA_SPIDER_BOOTSTRAP` | built-in list | Comma-separated DHT bootstrap nodes |
| `SERMA_CLEANUP` | enabled | Set to `0`, `false`, `off`, or `no` to disable cleanup |
| `SERMA_SOCKS5_PROXY` | (unset) | Optional SOCKS5 proxy for DHT UDP traffic (e.g. `socks5://127.0.0.1:1080` or `socks5://user:pass@host:1080`) |
| `SERMA_SOCKS5_USERNAME` | (unset) | SOCKS5 username (if not provided in URL) |
| `SERMA_SOCKS5_PASSWORD` | (unset) | SOCKS5 password (if not provided in URL) |
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

### Examples

**Run on custom port:**
```bash
SERMA_ADDR=0.0.0.0:8080 ./target/release/serma
```

**Use specific DHT port:**
```bash
SERMA_SPIDER_BIND=0.0.0.0:6881 ./target/release/serma
```

**Increase logging verbosity:**
```bash
RUST_LOG=debug ./target/release/serma
```

**Disable the DHT spider (search-only mode):**
```bash
SERMA_SPIDER=false ./target/release/serma
```

**Proxy DHT traffic via SOCKS5 (privacy):**
```bash
SERMA_SOCKS5_PROXY=socks5://127.0.0.1:1080 ./target/release/serma
```

## API Endpoints

Serma exposes a simple HTTP API:

### Search
```
GET /api/search?q=<query>&limit=<limit>&offset=<offset>
```

**Parameters:**
- `q`: Search query (required)
- `limit`: Results per page (default: 50, max: 500)
- `offset`: Pagination offset (default: 0)

**Response:**
```json
{
  "results": [
    {
      "info_hash": "abc123...",
      "title": "Example Torrent",
      "magnet": "magnet:?xt=urn:btih:...",
      "seeders": 42
    }
  ],
  "total": 1234,
  "limit": 50,
  "offset": 0
}
```

### Get Torrent by Hash
```
GET /api/torrent/<info_hash>
```

**Response:**
```json
{
  "info_hash": "abc123...",
  "title": "Example Torrent",
  "magnet": "magnet:?xt=urn:btih:...",
  "seeders": 42,
  "first_seen": 1704931200000,
  "last_seen": 1704931200000
}
```

## Data Storage

All data is stored in the `SERMA_DATA_DIR` (default: `./data`):

```
data/
├── sled/          # Embedded key-value database (torrent metadata)
└── tantivy/       # Full-text search index
```

**Backup**: Simply copy the entire `data/` directory to back up your index.

## How It Works

1. **Discovery**: The DHT spider joins the BitTorrent DHT network by connecting to bootstrap nodes
2. **Harvesting**: Listens for `announce_peer` and `get_peers` queries to discover info hashes
3. **Enrichment**: For each discovered hash:
   - Performs DHT peer lookup
   - Connects to peers and requests metadata via BEP-9 (ut_metadata)
   - Extracts torrent name and file information
4. **Indexing**: Stores metadata in Sled and indexes it in Tantivy for fast search
5. **Cleanup**: Periodically removes torrents with low seeders or inactivity

## Performance Notes

- **Initial seeding**: It may take 1-2 hours to discover your first 1,000 torrents
- **Index growth**: Expect ~10-50k new torrents per day depending on DHT traffic
- **Memory usage**: ~100-300 MB RAM typical, ~500 MB during heavy indexing
- **Disk I/O**: Mostly sequential writes, SSD recommended but not required

## Security Considerations

⚠️ **Important**: Serma is designed for **personal use only**. 

- This software interacts with the public BitTorrent DHT network
- You are discovering content that others are sharing; you are not hosting or distributing it
- Be aware of the legal implications in your jurisdiction
- Consider using a VPN if privacy is a concern
- Alternatively, set `SERMA_SOCKS5_PROXY` to route DHT UDP traffic via a SOCKS5 proxy
- **Do not** expose the web interface to the public internet without authentication

See [LICENSE](LICENSE) for the full disclaimer.

## Development

### Project Structure

```
src/
├── main.rs       # Application entry point
├── spider.rs     # DHT spider implementation
├── enrich.rs     # Metadata fetcher
├── index.rs      # Tantivy search index wrapper
├── storage.rs    # Sled database operations
├── cleanup.rs    # Cleanup task
└── web.rs        # Axum web server and UI
```

### Running in Development

```bash
cargo run
```

### Running Tests

```bash
cargo test
```

## Troubleshooting

### No torrents appearing in search

- **Check logs**: Ensure the spider is running (`RUST_LOG=debug`)
- **Wait**: Initial discovery can take 30-60 minutes
- **Network**: Ensure UDP traffic isn't blocked by firewall
- **DHT**: Try specifying a fixed port with `SERMA_SPIDER_BIND`

### High memory usage

- The in-memory bloom filter uses ~16 MB for deduplication
- Tantivy's index writer may use up to 500 MB during heavy writes
- Consider reducing `SERMA_SPIDER` traffic or increasing system resources

### Disk space filling up

- Serma includes automatic cleanup (default: every 10s; see `.env.example`)
- Adjust cleanup thresholds in `.env` if needed
- Manually delete `data/` and restart to reset the index

## Contributing

This is a personal project, but issues and pull requests are welcome for:
- Bug fixes
- Performance improvements
- Documentation improvements

Please note that this project is provided as-is with no warranty.

## License

See [LICENSE](LICENSE) file for details.

## Disclaimer

This software is provided for **educational and research purposes only**. The authors and contributors:
- Do not endorse, encourage, or facilitate copyright infringement
- Are not responsible for how you use this software
- Are not liable for any legal consequences resulting from its use
- Make no warranties about the software's fitness for any purpose

**Use at your own risk and in accordance with your local laws.**

---

Made with ❤️ and Rust
