use anyhow::Context;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

#[derive(Debug, Clone)]
pub struct Socks5Config {
    pub proxy: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Socks5Config {
    pub fn from_env() -> Option<anyhow::Result<Self>> {
        let proxy = std::env::var("SERMA_SOCKS5_PROXY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;

        let username = std::env::var("SERMA_SOCKS5_USERNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let password = std::env::var("SERMA_SOCKS5_PASSWORD")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Some(parse_proxy_string(&proxy).map(|(proxy_host_port, url_user, url_pass)| {
            Socks5Config {
                proxy: proxy_host_port,
                username: url_user.or(username),
                password: url_pass.or(password),
            }
        }))
    }

    async fn resolve_proxy_addr(&self) -> anyhow::Result<SocketAddr> {
        // Accept raw SocketAddr too (fast path).
        if let Ok(sa) = self.proxy.parse::<SocketAddr>() {
            return Ok(sa);
        }
        let mut iter = tokio::net::lookup_host(&self.proxy)
            .await
            .with_context(|| format!("resolve SOCKS5 proxy host: {}", self.proxy))?;
        iter.next()
            .with_context(|| format!("no addresses for SOCKS5 proxy: {}", self.proxy))
    }
}

/// A SOCKS5 UDP ASSOCIATE mapping.
///
/// Keeps the TCP control connection open so the proxy maintains the UDP mapping.
#[derive(Debug)]
pub struct Socks5UdpAssociate {
    udp: UdpSocket,
    relay: SocketAddr,
    _tcp: TcpStream,
}

impl Socks5UdpAssociate {
    pub async fn connect(cfg: &Socks5Config) -> anyhow::Result<Self> {
        let proxy_addr = cfg.resolve_proxy_addr().await?;

        let mut tcp = TcpStream::connect(proxy_addr)
            .await
            .with_context(|| format!("connect to SOCKS5 proxy: {proxy_addr}"))?;

        // 1) Greeting
        let want_userpass = cfg.username.is_some() || cfg.password.is_some();
        if want_userpass {
            tcp.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
        } else {
            tcp.write_all(&[0x05, 0x01, 0x00]).await?;
        }

        let mut choice = [0u8; 2];
        tcp.read_exact(&mut choice).await?;
        if choice[0] != 0x05 {
            anyhow::bail!("SOCKS5: invalid version in method select: {}", choice[0]);
        }
        match choice[1] {
            0x00 => {}
            0x02 => {
                let u = cfg.username.clone().unwrap_or_default();
                let p = cfg.password.clone().unwrap_or_default();
                if u.len() > 255 || p.len() > 255 {
                    anyhow::bail!("SOCKS5: username/password too long");
                }
                let mut auth = Vec::with_capacity(3 + u.len() + p.len());
                auth.push(0x01);
                auth.push(u.len() as u8);
                auth.extend_from_slice(u.as_bytes());
                auth.push(p.len() as u8);
                auth.extend_from_slice(p.as_bytes());
                tcp.write_all(&auth).await?;

                let mut resp = [0u8; 2];
                tcp.read_exact(&mut resp).await?;
                if resp[0] != 0x01 || resp[1] != 0x00 {
                    anyhow::bail!("SOCKS5: auth failed");
                }
            }
            0xFF => anyhow::bail!("SOCKS5: no acceptable auth methods"),
            other => anyhow::bail!("SOCKS5: unsupported auth method: {other}"),
        }

        // 2) UDP ASSOCIATE
        // Send an "unspecified" address of our IP family; proxy returns relay address.
        let mut req = Vec::with_capacity(32);
        req.extend_from_slice(&[0x05, 0x03, 0x00]);
        match proxy_addr.ip() {
            IpAddr::V4(_) => {
                req.push(0x01);
                req.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
                req.extend_from_slice(&0u16.to_be_bytes());
            }
            IpAddr::V6(_) => {
                req.push(0x04);
                req.extend_from_slice(&Ipv6Addr::UNSPECIFIED.octets());
                req.extend_from_slice(&0u16.to_be_bytes());
            }
        }
        tcp.write_all(&req).await?;

        let relay = read_socks5_reply_addr(&mut tcp)
            .await
            .context("SOCKS5: udp associate failed")?;

        let udp_bind = if relay.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let udp = UdpSocket::bind(udp_bind)
            .await
            .with_context(|| format!("bind UDP socket for SOCKS5 relay: {udp_bind}"))?;

        Ok(Self {
            udp,
            relay,
            _tcp: tcp,
        })
    }

    pub fn relay_addr(&self) -> SocketAddr {
        self.relay
    }

    pub async fn send_to(&self, payload: &[u8], target: SocketAddr) -> std::io::Result<usize> {
        let pkt = encode_udp_packet(target, payload);
        // Return the payload size to make callers treat this like a normal UDP socket.
        let _ = self.udp.send_to(&pkt, self.relay).await?;
        Ok(payload.len())
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        let (n, _from_relay) = self.udp.recv_from(buf).await?;
        let (src, payload_pos) = decode_udp_header(&buf[..n])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if payload_pos > n {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SOCKS5 UDP: invalid payload offset",
            ));
        }

        // Shift payload down so callers can treat this like a normal UDP socket.
        buf.copy_within(payload_pos..n, 0);
        Ok((n - payload_pos, src))
    }
}

fn parse_proxy_string(input: &str) -> anyhow::Result<(String, Option<String>, Option<String>)> {
    // Supported:
    // - host:port
    // - socks5://host:port
    // - socks5://user:pass@host:port
    let mut s = input.trim().to_string();

    if let Some(rest) = s.strip_prefix("socks5://") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("socks5h://") {
        s = rest.to_string();
    }

    // Strip any trailing path/query.
    if let Some((head, _)) = s.split_once('/') {
        s = head.to_string();
    }

    let (auth_part, host_part) = if let Some((a, h)) = s.rsplit_once('@') {
        (Some(a.to_string()), h.to_string())
    } else {
        (None, s)
    };

    let (user, pass) = if let Some(a) = auth_part {
        let (u, p) = a.split_once(':').unwrap_or((a.as_str(), ""));
        let u = u.trim().to_string();
        let p = p.trim().to_string();
        (
            (!u.is_empty()).then_some(u),
            (!p.is_empty()).then_some(p),
        )
    } else {
        (None, None)
    };

    // Validate host:port a bit early.
    let _ = parse_host_port(&host_part)
        .with_context(|| format!("invalid SERMA_SOCKS5_PROXY: {input}"))?;

    Ok((host_part, user, pass))
}

fn parse_host_port(hostport: &str) -> anyhow::Result<(&str, u16)> {
    // Accept bracketed IPv6.
    if let Some(rest) = hostport.strip_prefix('[') {
        let Some((host, rest)) = rest.split_once(']') else {
            anyhow::bail!("invalid IPv6 host:port");
        };
        let rest = rest.strip_prefix(':').context("missing port")?;
        let port: u16 = rest.parse().context("invalid port")?;
        return Ok((host, port));
    }

    let Some((host, port_str)) = hostport.rsplit_once(':') else {
        anyhow::bail!("missing port");
    };
    if host.is_empty() {
        anyhow::bail!("missing host");
    }
    let port: u16 = port_str.parse().context("invalid port")?;
    Ok((host, port))
}

async fn read_socks5_reply_addr(stream: &mut TcpStream) -> anyhow::Result<SocketAddr> {
    // VER REP RSV ATYP BND.ADDR BND.PORT
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        anyhow::bail!("SOCKS5: invalid reply version: {}", head[0]);
    }
    let rep = head[1];
    if rep != 0x00 {
        anyhow::bail!("SOCKS5: request failed (REP={rep:#x})");
    }
    let atyp = head[3];

    let addr = match atyp {
        0x01 => {
            let mut ip = [0u8; 4];
            stream.read_exact(&mut ip).await?;
            IpAddr::V4(Ipv4Addr::from(ip))
        }
        0x04 => {
            let mut ip = [0u8; 16];
            stream.read_exact(&mut ip).await?;
            IpAddr::V6(Ipv6Addr::from(ip))
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            stream.read_exact(&mut name).await?;
            let name = String::from_utf8_lossy(&name).to_string();
            // Resolve to first addr.
            let mut iter = tokio::net::lookup_host((name.as_str(), 0))
                .await
                .with_context(|| format!("resolve SOCKS5 reply domain: {name}"))?;
            iter.next()
                .context("SOCKS5: domain in reply resolved to no addresses")?
                .ip()
        }
        _ => anyhow::bail!("SOCKS5: unsupported ATYP in reply: {atyp}"),
    };

    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    Ok(SocketAddr::new(addr, port))
}

fn encode_udp_packet(target: SocketAddr, payload: &[u8]) -> Vec<u8> {
    // SOCKS5 UDP request header:
    // RSV(2) FRAG(1) ATYP(1) DST.ADDR DST.PORT DATA
    let mut out = Vec::with_capacity(64 + payload.len());
    out.extend_from_slice(&[0x00, 0x00, 0x00]);

    match target.ip() {
        IpAddr::V4(v4) => {
            out.push(0x01);
            out.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out.push(0x04);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&target.port().to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn decode_udp_header(pkt: &[u8]) -> Result<(SocketAddr, usize), &'static str> {
    if pkt.len() < 4 {
        return Err("SOCKS5 UDP: packet too short");
    }
    if pkt[0] != 0 || pkt[1] != 0 {
        return Err("SOCKS5 UDP: non-zero RSV");
    }
    if pkt[2] != 0 {
        return Err("SOCKS5 UDP: fragmentation not supported");
    }

    let atyp = pkt[3];
    let mut pos = 4usize;

    let ip = match atyp {
        0x01 => {
            if pkt.len() < pos + 4 {
                return Err("SOCKS5 UDP: truncated IPv4 addr");
            }
            let ip = Ipv4Addr::new(pkt[pos], pkt[pos + 1], pkt[pos + 2], pkt[pos + 3]);
            pos += 4;
            IpAddr::V4(ip)
        }
        0x04 => {
            if pkt.len() < pos + 16 {
                return Err("SOCKS5 UDP: truncated IPv6 addr");
            }
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&pkt[pos..pos + 16]);
            pos += 16;
            IpAddr::V6(Ipv6Addr::from(ip))
        }
        0x03 => {
            return Err("SOCKS5 UDP: domain ATYP not supported");
        }
        _ => return Err("SOCKS5 UDP: unsupported ATYP"),
    };

    if pkt.len() < pos + 2 {
        return Err("SOCKS5 UDP: missing port");
    }
    let port = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
    pos += 2;

    Ok((SocketAddr::new(ip, port), pos))
}
