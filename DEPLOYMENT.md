# Deployment

This guide shows a few practical ways to deploy Serma on a server.

Serma has two kinds of networking:
- **HTTP (TCP)** for the web UI + API
- **DHT spider (UDP)** for discovering hashes (optional but recommended)

Configuration is via environment variables (optionally from a `.env` file). See `.env.example` for the full list.

## Quick Notes

- If you expose Serma beyond localhost, **put it behind authentication** (reverse proxy auth, VPN, etc.).
- Backups are simple: **copy the entire data directory** (default `./data`, or `SERMA_DATA_DIR`).
- For best results, run on a machine that allows outbound UDP and (optionally) an inbound UDP port.

---

## Option A: Docker (Recommended)

### 1) Build the image

From the repo root:

```bash
docker build -t serma:latest .
```

### 2) Run it

This runs the web UI on port `3000` and binds the DHT UDP port `6881/udp`.

```bash
docker run -d \
  --name serma \
  --restart unless-stopped \
  -p 3000:3000 \
  -p 6881:6881/udp \
  -v serma-data:/data \
  -e RUST_LOG=info \
  -e SERMA_ADDR=0.0.0.0:3000 \
  -e SERMA_SPIDER_BIND=0.0.0.0:6881 \
  serma:latest
```

Open: `http://<server-ip>:3000`

### 3) Configure via env file (optional)

1. Create a local env file from the template:

```bash
cp .env.example .env
```

2. Edit `.env` and then run:

```bash
docker run -d \
  --name serma \
  --restart unless-stopped \
  -p 3000:3000 \
  -p 6881:6881/udp \
  -v serma-data:/data \
  --env-file ./.env \
  serma:latest
```

Notes:
- In Docker, `SERMA_DATA_DIR` is already set to `/data` by the Dockerfile.
- If you don’t want DHT crawling, disable it:
  - `SERMA_SPIDER=false`

### 4) Upgrade

```bash
docker stop serma
docker rm serma
docker build -t serma:latest .
# run the same docker run command again
```

Because your data is in a volume (`serma-data`), it will be reused.

### 5) Backup / restore

Backup (example creates a tarball of the Docker volume):

```bash
docker run --rm \
  -v serma-data:/data \
  -v "$PWD":/backup \
  debian:bookworm-slim \
  tar -C /data -czf /backup/serma-data.tgz .
```

Restore:

```bash
docker run --rm \
  -v serma-data:/data \
  -v "$PWD":/backup \
  debian:bookworm-slim \
  sh -lc 'rm -rf /data/* && tar -C /data -xzf /backup/serma-data.tgz'
```

---

## Option B: Linux + systemd (No Docker)

### 1) Build the binary

On the target server (or build elsewhere and copy the binary):

```bash
cargo build --release
```

Binary path:
- `./target/release/serma`

### 2) Create a dedicated user and directories

```bash
sudo useradd --system --create-home --home-dir /var/lib/serma --shell /usr/sbin/nologin serma
sudo mkdir -p /var/lib/serma/data
sudo chown -R serma:serma /var/lib/serma
sudo install -Dm755 ./target/release/serma /usr/local/bin/serma
```

### 3) Create an env file

```bash
sudo install -Dm600 /dev/null /etc/serma/serma.env
sudo cp .env.example /etc/serma/serma.env
sudo chown root:root /etc/serma/serma.env
sudo chmod 600 /etc/serma/serma.env
```

Edit it:

```bash
sudoedit /etc/serma/serma.env
```

At minimum, you probably want something like:

```bash
SERMA_DATA_DIR=/var/lib/serma/data
SERMA_ADDR=0.0.0.0:3000
SERMA_SPIDER_BIND=0.0.0.0:6881
RUST_LOG=info
```

### 4) Create a systemd unit

Create `/etc/systemd/system/serma.service`:

```ini
[Unit]
Description=Serma (local torrent index)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=serma
Group=serma
EnvironmentFile=/etc/serma/serma.env
WorkingDirectory=/var/lib/serma
ExecStart=/usr/local/bin/serma
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/serma

[Install]
WantedBy=multi-user.target
```

Enable + start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now serma
sudo systemctl status serma
```

Logs:

```bash
journalctl -u serma -f
```

### 5) Firewall

If you want external access:
- TCP `3000` for the web UI
- UDP `6881` for the spider

If you only want local access, bind HTTP to loopback:

```bash
SERMA_ADDR=127.0.0.1:3000
```

---

## Reverse Proxy (Recommended if public)

If you need remote access, prefer:
- Bind Serma to `127.0.0.1:3000`
- Put it behind a reverse proxy that enforces authentication (or only expose via VPN)

---

## Operational Tips

- **Disk growth**: the index grows over time; allocate tens of GB for meaningful indexing.
- **Resetting**: stopping Serma and deleting the data directory (or volume contents) resets the index.
- **Disabling crawler**: set `SERMA_SPIDER=false` if you only want to serve existing indexed data.
