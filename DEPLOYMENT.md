# Serma Deployment Guide

This guide covers various deployment scenarios for Serma, from local development to production servers.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Local Development](#local-development)
- [Production Deployment](#production-deployment)
  - [Systemd Service (Linux)](#systemd-service-linux)
  - [Docker Deployment](#docker-deployment)
  - [Reverse Proxy Setup](#reverse-proxy-setup)
- [Cloud Deployment](#cloud-deployment)
- [Performance Tuning](#performance-tuning)
- [Monitoring](#monitoring)
- [Backup and Recovery](#backup-and-recovery)
- [Security Hardening](#security-hardening)

---

## Prerequisites

### System Requirements

**Minimum:**
- 2 CPU cores
- 2 GB RAM
- 20 GB disk space
- Linux, macOS, or Windows

**Recommended:**
- 4+ CPU cores
- 4 GB RAM
- 50+ GB disk space (SSD preferred)
- Linux (Ubuntu 22.04+ or Debian 12+)
- Open UDP port for DHT traffic

### Software Requirements

- **Rust** 1.75+ (for building from source)
- **systemd** (for Linux service deployment)
- **Docker** (optional, for containerized deployment)
- **nginx** or **caddy** (optional, for reverse proxy)

---

## Local Development

### Quick Start

```bash
# Clone the repository
git clone <repository-url> serma
cd serma

# Build in debug mode
cargo build

# Run with default settings
cargo run

# Or build optimized release binary
cargo build --release
./target/release/serma
```

### Development Configuration

```bash
# Custom data directory
export SERMA_DATA_DIR=/tmp/serma-dev

# Enable debug logging
export RUST_LOG=debug

# Custom port
export SERMA_ADDR=127.0.0.1:8080

# Run
cargo run
```

---

## Production Deployment

### Build Optimized Binary

```bash
# Build with full optimizations
cargo build --release --locked

# Strip debug symbols (optional, reduces binary size)
strip target/release/serma

# Verify binary works
./target/release/serma --help
```

### Systemd Service (Linux)

#### 1. Create Service User

```bash
# Create a dedicated user for Serma
sudo useradd -r -s /bin/false -d /var/lib/serma serma

# Create data directory
sudo mkdir -p /var/lib/serma/data
sudo chown -R serma:serma /var/lib/serma
sudo chmod 750 /var/lib/serma
```

#### 2. Install Binary

```bash
# Copy binary to system location
sudo cp target/release/serma /usr/local/bin/
sudo chown root:root /usr/local/bin/serma
sudo chmod 755 /usr/local/bin/serma
```

#### 3. Create Systemd Service File

Create `/etc/systemd/system/serma.service`:

```ini
[Unit]
Description=Serma - BitTorrent DHT Search Engine
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=serma
Group=serma
WorkingDirectory=/var/lib/serma

# Environment variables
Environment="SERMA_DATA_DIR=/var/lib/serma/data"
Environment="SERMA_ADDR=127.0.0.1:3000"
Environment="SERMA_SPIDER_BIND=0.0.0.0:6881"
Environment="RUST_LOG=info"

# Security hardening
PrivateTmp=yes
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/serma/data

# Resource limits
LimitNOFILE=65536
MemoryMax=2G

# Restart policy
Restart=on-failure
RestartSec=10
KillMode=mixed
KillSignal=SIGTERM
TimeoutStopSec=30

# Execute
ExecStart=/usr/local/bin/serma

[Install]
WantedBy=multi-user.target
```

#### 4. Enable and Start Service

```bash
# Reload systemd
sudo systemctl daemon-reload

# Enable service to start on boot
sudo systemctl enable serma

# Start service
sudo systemctl start serma

# Check status
sudo systemctl status serma

# View logs
sudo journalctl -u serma -f
```

#### 5. Firewall Configuration

```bash
# Allow DHT UDP port (if using fixed port)
sudo ufw allow 6881/udp

# Allow HTTP (if exposing directly, not recommended)
# sudo ufw allow 3000/tcp
```

---

### Docker Deployment

#### 1. Create Dockerfile

Create `Dockerfile` in project root:

```dockerfile
# Build stage
FROM rust:1.75-slim AS builder

WORKDIR /build

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build with optimizations
RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create user
RUN useradd -r -s /bin/false -u 1000 serma

# Copy binary
COPY --from=builder /build/target/release/serma /usr/local/bin/serma

# Create data directory
RUN mkdir -p /data && chown serma:serma /data

# Switch to non-root user
USER serma

# Set working directory
WORKDIR /data

# Expose ports
EXPOSE 3000/tcp 6881/udp

# Set environment defaults
ENV SERMA_DATA_DIR=/data \
    SERMA_ADDR=0.0.0.0:3000 \
    SERMA_SPIDER_BIND=0.0.0.0:6881 \
    RUST_LOG=info

# Run
CMD ["/usr/local/bin/serma"]
```

#### 2. Create docker-compose.yml

```yaml
version: '3.8'

services:
  serma:
    build: .
    container_name: serma
    restart: unless-stopped
    
    ports:
      - "127.0.0.1:3000:3000"  # HTTP (bind to localhost)
      - "6881:6881/udp"         # DHT
    
    volumes:
      - ./data:/data
    
    environment:
      - SERMA_DATA_DIR=/data
      - SERMA_ADDR=0.0.0.0:3000
      - SERMA_SPIDER_BIND=0.0.0.0:6881
      - RUST_LOG=info
    
    # Resource limits
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 2G
        reservations:
          cpus: '1'
          memory: 512M
    
    # Health check
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
```

#### 3. Build and Run

```bash
# Build image
docker-compose build

# Start in background
docker-compose up -d

# View logs
docker-compose logs -f

# Stop
docker-compose down
```

---

### Reverse Proxy Setup

#### Nginx Configuration

Create `/etc/nginx/sites-available/serma`:

```nginx
upstream serma {
    server 127.0.0.1:3000;
    keepalive 32;
}

server {
    listen 80;
    listen [::]:80;
    server_name serma.example.com;

    # Redirect to HTTPS
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name serma.example.com;

    # SSL certificates (use certbot/letsencrypt)
    ssl_certificate /etc/letsencrypt/live/serma.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/serma.example.com/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers off;

    # Security headers
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header X-XSS-Protection "1; mode=block" always;

    # Access control (IMPORTANT!)
    # Option 1: IP whitelist
    # allow 192.168.1.0/24;
    # deny all;

    # Option 2: HTTP Basic Auth
    # auth_basic "Serma Access";
    # auth_basic_user_file /etc/nginx/.htpasswd;

    location / {
        proxy_pass http://serma;
        proxy_http_version 1.1;
        
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;
    }

    # Rate limiting
    limit_req_zone $binary_remote_addr zone=serma_limit:10m rate=10r/s;
    limit_req zone=serma_limit burst=20 nodelay;
}
```

Enable and restart:

```bash
# Create symlink
sudo ln -s /etc/nginx/sites-available/serma /etc/nginx/sites-enabled/

# Test configuration
sudo nginx -t

# Reload nginx
sudo systemctl reload nginx

# Get SSL certificate (if using Let's Encrypt)
sudo certbot --nginx -d serma.example.com
```

#### Caddy Configuration (Simpler Alternative)

Create `Caddyfile`:

```caddy
serma.example.com {
    # Automatic HTTPS via Let's Encrypt
    
    # Basic authentication (recommended)
    basicauth {
        admin $2a$14$HxVV9z.xyz...  # Use 'caddy hash-password' to generate
    }
    
    # Or IP whitelist
    # @allowed {
    #     remote_ip 192.168.1.0/24
    # }
    # handle @allowed {
    #     reverse_proxy localhost:3000
    # }
    # respond 403
    
    reverse_proxy localhost:3000
    
    # Rate limiting
    rate_limit {
        zone dynamic {
            key {remote_host}
            events 100
            window 1m
        }
    }
}
```

Run Caddy:

```bash
caddy run --config Caddyfile
```

---

## Cloud Deployment

### AWS EC2

1. **Launch Instance:**
   - Ubuntu 22.04 LTS
   - t3.medium or larger
   - 20+ GB storage
   - Security group: Allow 22/tcp (SSH), 6881/udp (DHT)

2. **Setup:**

```bash
# SSH into instance
ssh ubuntu@<instance-ip>

# Update system
sudo apt update && sudo apt upgrade -y

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Clone and build Serma
git clone <repository-url> serma
cd serma
cargo build --release

# Follow systemd service setup from above
```

### DigitalOcean Droplet

Similar to AWS EC2:
- Use Ubuntu 22.04 droplet
- 2 GB RAM minimum
- Add firewall rule for UDP 6881

### VPS (Hetzner, Linode, etc.)

Same steps as AWS/DigitalOcean. Ensure:
- At least 2 GB RAM
- Unrestricted UDP traffic for DHT
- Consider using block storage for data directory

---

## Performance Tuning

### System Tuning

```bash
# Increase file descriptor limits
echo "* soft nofile 65536" | sudo tee -a /etc/security/limits.conf
echo "* hard nofile 65536" | sudo tee -a /etc/security/limits.conf

# Increase network buffer sizes (helps DHT spider)
sudo sysctl -w net.core.rmem_max=26214400
sudo sysctl -w net.core.wmem_max=26214400
sudo sysctl -w net.core.rmem_default=26214400
sudo sysctl -w net.core.wmem_default=26214400

# Make persistent
echo "net.core.rmem_max = 26214400" | sudo tee -a /etc/sysctl.conf
echo "net.core.wmem_max = 26214400" | sudo tee -a /etc/sysctl.conf
```

### Database Optimization

Sled and Tantivy automatically tune themselves, but consider:

- **SSD storage**: Dramatically improves indexing speed
- **Separate volume**: Mount data directory on separate disk
- **Memory**: More RAM = better caching

### Spider Tuning

Modify constants in `spider.rs` for your needs:

```rust
const MAX_KNOWN_NODES: usize = 10_000;  // Increase for more coverage
const SAMPLE_PER_TICK: usize = 12;      // Increase for faster discovery
```

Rebuild after changes:

```bash
cargo build --release
```

---

## Monitoring

### Logs

```bash
# Systemd service logs
sudo journalctl -u serma -f

# Docker logs
docker-compose logs -f

# Filter by level
sudo journalctl -u serma -p err -f  # Errors only
```

### Metrics to Watch

- **Discovery rate**: New info hashes per minute (check logs)
- **Enrichment success**: Metadata fetch success rate
- **Disk usage**: `du -sh /var/lib/serma/data`
- **Memory usage**: `ps aux | grep serma`
- **CPU usage**: `top -p $(pidof serma)`

### Health Check Script

Create `health_check.sh`:

```bash
#!/bin/bash
ENDPOINT="http://localhost:3000/"
TIMEOUT=5

if curl -sf --max-time $TIMEOUT "$ENDPOINT" > /dev/null; then
    echo "Serma is healthy"
    exit 0
else
    echo "Serma is unhealthy"
    exit 1
fi
```

Add to cron for periodic checks:

```bash
# Check every 5 minutes
*/5 * * * * /usr/local/bin/health_check.sh || sudo systemctl restart serma
```

---

## Backup and Recovery

### Backup Strategy

```bash
#!/bin/bash
# backup.sh - Simple backup script

BACKUP_DIR="/backups/serma"
DATA_DIR="/var/lib/serma/data"
DATE=$(date +%Y%m%d_%H%M%S)

# Stop service for consistent backup
sudo systemctl stop serma

# Create backup
mkdir -p "$BACKUP_DIR"
tar -czf "$BACKUP_DIR/serma_backup_$DATE.tar.gz" -C "$(dirname $DATA_DIR)" "$(basename $DATA_DIR)"

# Start service
sudo systemctl start serma

# Keep only last 7 backups
ls -t "$BACKUP_DIR"/serma_backup_*.tar.gz | tail -n +8 | xargs rm -f

echo "Backup completed: serma_backup_$DATE.tar.gz"
```

### Restore from Backup

```bash
# Stop service
sudo systemctl stop serma

# Restore data
sudo tar -xzf /backups/serma/serma_backup_YYYYMMDD_HHMMSS.tar.gz -C /var/lib/serma/

# Fix permissions
sudo chown -R serma:serma /var/lib/serma/data

# Start service
sudo systemctl start serma
```

### Incremental Backups

Use rsync for efficient incremental backups:

```bash
rsync -avz --delete /var/lib/serma/data/ backup-server:/backups/serma/
```

---

## Security Hardening

### ⚠️ Critical Security Measures

1. **Never expose to public internet without authentication**
2. **Use reverse proxy with auth (basic auth or OAuth)**
3. **Bind to localhost** if behind reverse proxy
4. **Keep system updated**: `sudo apt update && sudo apt upgrade`
5. **Use firewall**: Only allow necessary ports
6. **Monitor logs**: Watch for suspicious activity

### Recommended: Tailscale/Wireguard

For secure remote access without public exposure:

```bash
# Install Tailscale
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up

# Access Serma via Tailscale IP
# No need to expose to public internet
```

### HTTP Basic Auth with Nginx

```bash
# Create password file
sudo htpasswd -c /etc/nginx/.htpasswd admin

# Configure in nginx (see above)
```

---

## Troubleshooting

### Service Won't Start

```bash
# Check logs
sudo journalctl -u serma -n 50

# Check if port is in use
sudo netstat -tulpn | grep 3000

# Test binary manually
sudo -u serma /usr/local/bin/serma
```

### No DHT Traffic

```bash
# Check firewall
sudo ufw status

# Verify UDP port is listening
sudo netstat -ulpn | grep 6881

# Test with verbose logging
RUST_LOG=debug /usr/local/bin/serma
```

### High Disk Usage

```bash
# Check disk usage
du -sh /var/lib/serma/data/*

# Clean up and restart (resets index)
sudo systemctl stop serma
sudo rm -rf /var/lib/serma/data/*
sudo systemctl start serma
```

### Performance Issues

```bash
# Check system resources
htop

# Check I/O wait
iostat -x 1

# Consider moving to SSD or increasing RAM
```

---

## Updates and Maintenance

### Updating Serma

```bash
# Pull latest code
cd serma
git pull

# Rebuild
cargo build --release

# Stop service
sudo systemctl stop serma

# Replace binary
sudo cp target/release/serma /usr/local/bin/

# Start service
sudo systemctl start serma

# Check logs
sudo journalctl -u serma -f
```

### Maintenance Schedule

- **Daily**: Check logs for errors
- **Weekly**: Review disk usage
- **Monthly**: Update system packages
- **Quarterly**: Review and optimize cleanup settings

---

## Support

For issues and questions:
- Check logs first: `sudo journalctl -u serma`
- Review this guide
- Check GitHub issues
- Remember: This is provided as-is with no warranty

---

**Last Updated**: January 2026
