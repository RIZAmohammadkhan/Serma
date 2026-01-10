# Serma

Single-binary BitTorrent metadata crawler + local search index.

Serma:
- Ingests torrent info-hashes (40 hex chars)
- Enriches them in-process via DHT + `ut_metadata` (stores the full bencoded `info` dictionary)
- Indexes/searches via Tantivy
- Serves a small HTML UI + JSON API over HTTP

## Quickstart

### Run (dev)

```bash
cargo run
```

Open:
- http://127.0.0.1:3000

### Run (release, single binary)

```bash
cargo build --release
./target/release/serma
```

## Configuration

Environment variables:

- `SERMA_ADDR` (default: `127.0.0.1:3000`)
  - Example: `SERMA_ADDR=0.0.0.0:3000`

- `SERMA_DATA_DIR` (default: `data`)
  - Stores:
    - Sled DB under `${SERMA_DATA_DIR}/sled/`
    - Tantivy index under `${SERMA_DATA_DIR}/tantivy/`
    - Optional ingest file `${SERMA_DATA_DIR}/hashes.txt`

Example:

```bash
SERMA_ADDR=0.0.0.0:3000 SERMA_DATA_DIR=/var/lib/serma ./target/release/serma
```

## Ingestion

On startup, Serma reads **one info-hash per line** from:

1. `${SERMA_DATA_DIR}/hashes.txt` (if it exists), otherwise
2. `stdin`

Lines that are not exactly 40 hex characters are skipped.

Examples:

```bash
# Ingest from stdin
cat hashes.txt | SERMA_DATA_DIR=data ./target/release/serma

# Or place hashes in the default file location
mkdir -p data
cp hashes.txt data/hashes.txt
./target/release/serma
```

## Enrichment (in-binary)

A background worker continuously scans the DB for records missing metadata and attempts to enrich them:

- DHT `get_peers(info_hash)` to find peers
- BEP-10 extension handshake
- `ut_metadata` piece download
- Persists the full bencoded `info` dictionary (base64-encoded) in sled
- Updates Tantivy (upsert by `info_hash`) and refreshes search ranking by seeders

Enrichment is best-effort: peers may be offline, slow, or not support `ut_metadata`.

## Web UI + API

- `GET /` Home/search
- `GET /search?q=...` HTML results
- `GET /t/:info_hash` HTML torrent details
- `GET /api/search?q=...` JSON search results

## Notes

- This is an MVP: data quality depends on network conditions and peer availability.
- On first run, enrichment may take time before metadata appears on detail pages.
