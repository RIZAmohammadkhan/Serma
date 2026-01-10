use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

const APP_TITLE: &str = "Serma";
const APP_TAGLINE: &str = "The local index.";

// Icons (Stroke width adjusted for dark mode contrast)
const ICON_MAGNET: &str = r##"<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3a2 2 0 0 0-2 2v7a7 7 0 0 0 14 0V5a2 2 0 0 0-2-2h-2v9a3 3 0 0 1-6 0V3H7Z"/><path d="M9 3v9a3 3 0 0 0 6 0V3" opacity="0.5"/></svg>"##;
const ICON_COPY: &str = r##"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>"##;
const ICON_SEARCH: &str = r##"<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>"##;
const ICON_ARROW_RIGHT: &str = r##"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"/><polyline points="12 5 19 12 12 19"/></svg>"##;

fn page(title: &str, body: String) -> Html<String> {
    let full_title = if title.trim().is_empty() {
        APP_TITLE.to_string()
    } else {
        format!("{} / {}", title, APP_TITLE)
    };

    Html(format!(
        r##"<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="color-scheme" content="dark" />
    <title>{}</title>
    <style>
        :root {{
            --bg: #09090b;
            --surface: #121214;
            --surface-hover: #1c1c1f;
            --border: #27272a;
            --border-hover: #3f3f46;
            
            --text-main: #ededed;
            --text-muted: #a1a1aa;
            --text-faint: #52525b;
            
            --accent: #fff;
            --accent-bg: #fff;
            --accent-text: #000;
            
            --font-sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            --font-mono: "JetBrains Mono", "SF Mono", Consolas, Menlo, monospace;
            
            --radius: 6px;
            --container-width: 800px;
        }}

        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        
        body {{
            background-color: var(--bg);
            color: var(--text-main);
            font-family: var(--font-sans);
            font-size: 14px;
            line-height: 1.5;
            -webkit-font-smoothing: antialiased;
            display: flex;
            flex-direction: column;
            min-height: 100vh;
        }}

        a {{ text-decoration: none; color: inherit; transition: color 0.2s; }}
        a:hover {{ color: var(--accent); }}

        /* Utility */
        .container {{
            width: 100%;
            max-width: var(--container-width);
            margin: 0 auto;
            padding: 0 24px;
        }}
        .flex {{ display: flex; align-items: center; }}
        .gap-2 {{ gap: 8px; }}
        .gap-4 {{ gap: 16px; }}
        .mono {{ font-family: var(--font-mono); font-size: 0.9em; }}
        .muted {{ color: var(--text-muted); }}

        /* Navigation */
        header {{
            border-bottom: 1px solid var(--border);
            padding: 16px 0;
            position: sticky;
            top: 0;
            background: rgba(9, 9, 11, 0.8);
            backdrop-filter: blur(8px);
            z-index: 10;
        }}
        .nav-inner {{
            display: flex;
            justify-content: space-between;
            align-items: center;
        }}
        .brand {{
            font-weight: 700;
            font-size: 16px;
            letter-spacing: -0.02em;
            display: flex;
            align-items: center;
            gap: 10px;
        }}
        .brand-dot {{
            width: 8px;
            height: 8px;
            background: var(--accent);
            border-radius: 50%;
        }}

        /* Inputs & Forms */
        .search-wrapper {{
            position: relative;
            width: 100%;
        }}
        input[type="text"] {{
            width: 100%;
            background: var(--surface);
            border: 1px solid var(--border);
            color: var(--text-main);
            padding: 12px 16px;
            border-radius: var(--radius);
            font-size: 15px;
            transition: all 0.2s ease;
            font-family: var(--font-sans);
        }}
        input[type="text"]:focus {{
            outline: none;
            border-color: var(--text-muted);
            background: var(--surface-hover);
        }}
        
        /* Buttons */
        .btn {{
            display: inline-flex;
            align-items: center;
            justify-content: center;
            gap: 8px;
            padding: 8px 16px;
            border-radius: var(--radius);
            font-weight: 500;
            font-size: 13px;
            cursor: pointer;
            transition: all 0.2s;
            border: 1px solid transparent;
        }}
        .btn-primary {{
            background: var(--accent-bg);
            color: var(--accent-text);
        }}
        .btn-primary:hover {{
            opacity: 0.9;
        }}
        .btn-ghost {{
            background: transparent;
            border: 1px solid var(--border);
            color: var(--text-main);
        }}
        .btn-ghost:hover {{
            background: var(--surface-hover);
            border-color: var(--border-hover);
        }}
        .btn-icon {{
            padding: 8px;
            color: var(--text-muted);
        }}
        .btn-icon:hover {{
            color: var(--text-main);
            background: var(--surface-hover);
            border-radius: var(--radius);
        }}

        /* Lists & Cards */
        .results-list {{
            list-style: none;
            margin-top: 24px;
            display: flex;
            flex-direction: column;
            gap: 1px; /* Divider look */
            background: var(--border); /* Creates lines between items */
            border: 1px solid var(--border);
            border-radius: var(--radius);
            overflow: hidden;
        }}
        .list-item {{
            background: var(--bg);
            padding: 16px 20px;
            display: flex;
            flex-direction: column;
            gap: 8px;
            transition: background 0.15s;
        }}
        .list-item:hover {{
            background: var(--surface);
        }}
        .item-header {{
            display: flex;
            justify-content: space-between;
            align-items: flex-start;
            gap: 16px;
        }}
        .item-title {{
            font-weight: 500;
            font-size: 15px;
            color: var(--text-main);
            line-height: 1.4;
        }}
        .item-meta {{
            display: flex;
            gap: 16px;
            font-size: 12px;
            color: var(--text-muted);
            align-items: center;
            margin-top: 4px;
        }}
        .badge {{
            display: inline-block;
            padding: 2px 6px;
            border-radius: 4px;
            background: var(--surface-hover);
            border: 1px solid var(--border);
            font-size: 11px;
            font-weight: 600;
            color: var(--text-muted);
            font-family: var(--font-mono);
        }}
        
        /* Hero Section */
        .hero {{
            padding: 80px 0;
            text-align: center;
            display: flex;
            flex-direction: column;
            align-items: center;
            gap: 24px;
        }}
        .hero h2 {{
            font-size: 32px;
            font-weight: 600;
            letter-spacing: -0.03em;
        }}
        .hero p {{
            color: var(--text-muted);
            max-width: 460px;
            font-size: 16px;
        }}
        .hero-search {{
            width: 100%;
            max-width: 500px;
            margin-top: 16px;
        }}

        /* Detail Page */
        .detail-card {{
            margin-top: 32px;
            border: 1px solid var(--border);
            border-radius: var(--radius);
            background: var(--surface);
            padding: 32px;
        }}
        .detail-header {{
            border-bottom: 1px solid var(--border);
            padding-bottom: 24px;
            margin-bottom: 24px;
        }}
        .detail-title {{
            font-size: 20px;
            font-weight: 600;
            margin-bottom: 12px;
        }}
        .magnet-box {{
            background: var(--bg);
            border: 1px solid var(--border);
            border-radius: var(--radius);
            padding: 4px;
            display: flex;
            gap: 8px;
            margin-top: 16px;
        }}
        .magnet-box input {{
            border: none;
            background: transparent;
            font-family: var(--font-mono);
            font-size: 12px;
            color: var(--text-muted);
        }}
        .magnet-box input:focus {{ background: transparent; }}

        /* Footer */
        footer {{
            margin-top: auto;
            border-top: 1px solid var(--border);
            padding: 32px 0;
            color: var(--text-faint);
            font-size: 13px;
            text-align: center;
        }}
        
        /* Toast */
        .toast {{
            position: fixed;
            bottom: 24px;
            right: 24px;
            background: var(--text-main);
            color: var(--bg);
            padding: 10px 16px;
            border-radius: var(--radius);
            font-weight: 500;
            font-size: 13px;
            transform: translateY(100px);
            opacity: 0;
            transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
            pointer-events: none;
            z-index: 100;
        }}
        .toast.show {{ transform: translateY(0); opacity: 1; }}

        /* Mobile */
        @media (max-width: 600px) {{
            :root {{ --container-width: 100%; }}
            .hero {{ padding: 40px 0; }}
            .hero h2 {{ font-size: 24px; }}
            .item-header {{ flex-direction: column; gap: 8px; }}
        }}
    </style>
</head>
<body>
    <header>
        <div class="container nav-inner">
            <a href="/" class="brand">
                <div class="brand-dot"></div>
                {}
            </a>
            <nav class="flex gap-4">
                <a href="/" class="muted" style="font-size:13px;">Home</a>
                <a href="/search" class="muted" style="font-size:13px;">Browse</a>
            </nav>
        </div>
    </header>

    <div class="container">
        {}
    </div>

    <footer>
        <div class="container">
            <p>Local torrent indexing &middot; Data persists in <code>data/</code></p>
        </div>
    </footer>

    <div id="toast" class="toast">Notification</div>

    <script>
        const toastEl = document.getElementById('toast');
        let toastTimeout;
        
        function showToast(msg) {{
            toastEl.textContent = msg;
            toastEl.classList.add('show');
            clearTimeout(toastTimeout);
            toastTimeout = setTimeout(() => toastEl.classList.remove('show'), 2000);
        }}

        document.addEventListener('click', async (e) => {{
            const btn = e.target.closest('[data-copy]');
            if (!btn) return;
            e.preventDefault();
            
            const text = btn.getAttribute('data-copy');
            if (!text) return;
            
            try {{
                await navigator.clipboard.writeText(text);
                showToast('Copied to clipboard');
            }} catch (err) {{
                // Fallback
                const ta = document.createElement('textarea');
                ta.value = text;
                document.body.appendChild(ta);
                ta.select();
                document.execCommand('copy');
                document.body.removeChild(ta);
                showToast('Copied');
            }}
        }});
    </script>
</body>
</html>"##,
        html_escape(&full_title),
        html_escape(APP_TITLE),
        body
    ))
}

pub async fn serve(state: AppState, addr: std::net::SocketAddr) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(home))
        .route("/search", get(search_html))
        .route("/search/", get(search_html))
        .route("/api/search", get(search_api))
        .route("/api/search/", get(search_api))
        .route("/t/:info_hash", get(torrent_page))
        .with_state(state);
    tracing::info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

pub async fn serve_dual_loopback(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(home))
        .route("/search", get(search_html))
        .route("/search/", get(search_html))
        .route("/api/search", get(search_api))
        .route("/api/search/", get(search_api))
        .route("/t/:info_hash", get(torrent_page))
        .with_state(state);

    let addr_v4: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse()?;
    tracing::info!(%addr_v4, "listening");
    let listener_v4 = tokio::net::TcpListener::bind(addr_v4).await?;
    let server_v4 = axum::serve(listener_v4, app.clone());

    let addr_v6: std::net::SocketAddr = format!("[::1]:{}", port).parse()?;
    let listener_v6 = match tokio::net::TcpListener::bind(addr_v6).await {
        Ok(l) => {
            tracing::info!(%addr_v6, "listening");
            Some(l)
        }
        Err(err) => {
            tracing::debug!(%err, %addr_v6, "ipv6 bind failed; continuing with ipv4 only");
            None
        }
    };

    if let Some(listener_v6) = listener_v6 {
        let server_v6 = axum::serve(listener_v6, app);
        tokio::select! {
            r = server_v4 => r?,
            r = server_v6 => r?,
        }
    } else {
        server_v4.await?;
    }

    Ok(())
}

async fn home() -> impl IntoResponse {
    page(
        "Home",
        format!(
            r##"
            <main class="hero">
                <h2>Local Index</h2>
                <p>Serma continuously discovers hashes, enriches metadata, and ranks by seeders locally.</p>
                <form action="/search" method="get" class="hero-search">
                    <div class="search-wrapper">
                        <input type="text" name="q" placeholder="Search by title..." autocomplete="off" autofocus />
                    </div>
                    <div style="margin-top: 16px; display: flex; gap: 8px; justify-content: center;">
                         <button type="submit" class="btn btn-primary">{} Search</button>
                    </div>
                </form>
            </main>
            "##,
            ICON_SEARCH
        ),
    )
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
}

async fn search_html(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    let hits = if q.trim().is_empty() {
        Vec::new()
    } else {
        state.index.search(&q, 25).unwrap_or_default()
    };

    let mut items = String::new();
    for hit in hits {
        let info_hash = hit.info_hash.unwrap_or_default();
        let title = hit.title.unwrap_or_else(|| "(untitled)".to_string());
        let magnet = hit.magnet.unwrap_or_default();
        let short_hash = if info_hash.len() > 12 {
            &info_hash[0..12]
        } else {
            &info_hash
        };

        let actions = if !magnet.is_empty() {
            format!(
                r##"<a href="{}" class="btn btn-icon" title="Magnet">{}</a>
                   <button class="btn btn-icon" data-copy="{}" title="Copy Link">{}</button>"##,
                html_escape(&magnet),
                ICON_MAGNET,
                html_escape(&magnet),
                ICON_COPY
            )
        } else {
            String::new()
        };

        items.push_str(&format!(
            r##"
            <li class="list-item">
                <div class="item-header">
                    <div>
                        <a href="/t/{}" class="item-title">{}</a>
                        <div class="item-meta">
                            <span class="badge">S: {}</span>
                            <span class="mono">#{}</span>
                        </div>
                    </div>
                    <div class="flex">
                        {}
                        <a href="/t/{}" class="btn btn-icon">{}</a>
                    </div>
                </div>
            </li>
            "##,
            html_escape(&info_hash),
            html_escape(&title),
            hit.seeders,
            html_escape(short_hash),
            actions,
            html_escape(&info_hash),
            ICON_ARROW_RIGHT
        ));
    }

    let results_html = if items.is_empty() {
        r##"<div style="text-align:center; padding: 40px; color: var(--text-muted);">No results found.</div>"##
            .to_string()
    } else {
        format!("<ul class=\"results-list\">{}</ul>", items)
    };

    page(
        &q,
        format!(
            r##"
            <div style="margin-top: 32px;">
                <form action="/search" method="get" class="search-wrapper">
                    <input type="text" name="q" value="{}" placeholder="Search..." autocomplete="off" />
                </form>
                {}
            </div>
            "##,
            html_escape(&q),
            results_html
        ),
    )
}

async fn search_api(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    let hits = if q.trim().is_empty() {
        Vec::new()
    } else {
        state.index.search(&q, 25).unwrap_or_default()
    };
    Json(hits)
}

async fn torrent_page(
    State(state): State<AppState>,
    Path(info_hash): Path<String>,
) -> impl IntoResponse {
    let record = crate::storage::get(&state.db, &info_hash).ok().flatten();

    let title = record
        .as_ref()
        .and_then(|r| r.title.clone())
        .unwrap_or_else(|| "Unknown Title".to_string());

    let magnet = record
        .as_ref()
        .and_then(|r| r.magnet.clone())
        .unwrap_or_default();

    let seeders = record.as_ref().map(|r| r.seeders).unwrap_or(0);
    
    let magnet_section = if magnet.is_empty() {
        String::new()
    } else {
        format!(
            r##"
            <div class="magnet-box">
                <div class="flex" style="padding: 0 12px; color: var(--text-muted);">{}</div>
                <input type="text" value="{}" readonly onclick="this.select()" />
                <button class="btn btn-ghost" data-copy="{}">Copy</button>
                <a href="{}" class="btn btn-primary">Open</a>
            </div>
            "##,
            ICON_MAGNET,
            html_escape(&magnet),
            html_escape(&magnet),
            html_escape(&magnet)
        )
    };

    page(
        &title,
        format!(
            r##"
            <main class="detail-card">
                <div class="detail-header">
                    <div style="color: var(--text-muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 8px;">Torrent Detail</div>
                    <h1 class="detail-title">{}</h1>
                    <div class="flex gap-4">
                        <span class="badge">Seeders: {}</span>
                        <span class="mono muted">{}</span>
                    </div>
                </div>
                
                <div style="margin-top: 24px;">
                    <h3 style="font-size: 14px; font-weight: 500; margin-bottom: 8px;">Magnet Link</h3>
                    {}
                </div>

                <div style="margin-top: 32px;">
                    <a href="/search" class="btn btn-ghost" style="display:inline-flex;">&larr; Back to Search</a>
                </div>
            </main>
            "##,
            html_escape(&title),
            seeders,
            html_escape(&info_hash),
            magnet_section
        ),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}