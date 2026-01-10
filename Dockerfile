# ---------------------------------------------------
# 1. Build Stage
# ---------------------------------------------------
FROM rust:1.83-slim-bookworm as builder

WORKDIR /usr/src/app

# Install build dependencies (OpenSSL is required for many crates)
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifest files
COPY Cargo.toml ./

# Copy source code
COPY src ./src

# Build the release binary
RUN cargo build --release --locked

# ---------------------------------------------------
# 2. Runtime Stage
# ---------------------------------------------------
FROM debian:bookworm-slim

# Install runtime dependencies (SSL certs for HTTPS trackers)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user for security
RUN useradd -ms /bin/bash serma

# Create data directory and set permissions
RUN mkdir -p /data && chown serma:serma /data

# Switch to user
USER serma
WORKDIR /app

# Copy the binary from the builder stage
COPY --from=builder /usr/src/app/target/release/serma /usr/local/bin/serma

# Set environment variables
ENV RUST_LOG=info
ENV SERMA_DATA_DIR=/data
ENV SERMA_ADDR=0.0.0.0:3000
ENV SERMA_SPIDER_BIND=0.0.0.0:6881

# Expose ports (Web UI and DHT UDP)
EXPOSE 3000
EXPOSE 6881/udp

# Run the binary
CMD ["serma"]