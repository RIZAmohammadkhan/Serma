use crate::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Deserialize;

const APP_TITLE: &str = "Serma";
const APP_TAGLINE: &str = "Local torrent search, continuously enriched.";

const ICON_MAGNET: &str = r#"<svg class="icon" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M7 3a2 2 0 0 0-2 2v7a7 7 0 0 0 14 0V5a2 2 0 0 0-2-2h-2v9a3 3 0 0 1-6 0V3H7Z" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/><path d="M9 3v9a3 3 0 0 0 6 0V3" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" opacity="0.55"/></svg>"#;
const ICON_COPY: &str = r#"<svg class="icon" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M9 9h10v11H9V9Z" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/><path d="M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>"#;
const ICON_SEARCH: &str = r#"<svg class="icon" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><path d="M10.5 18a7.5 7.5 0 1 1 0-15 7.5 7.5 0 0 1 0 15Z" stroke="currentColor" stroke-width="1.7"/><path d="M21 21l-4.2-4.2" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>"#;

fn page(title: &str, body: String) -> Html<String> {
    let full_title = if title.trim().is_empty() {
        APP_TITLE.to_string()
    } else {
        format!("{} · {}", title, APP_TITLE)
    };

    Html(format!(
        r##"<!doctype html>
<html lang="en">
    <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="color-scheme" content="light" />
        <title>{}</title>
        <style>
            :root {{
                /* Coffee / chocolate / beige palette (minimal, elegant) */
                --bg-primary: #fbf6ef;
                --bg-secondary: #f3ece3;
                --surface: rgba(255, 255, 255, 0.72);
                --surface-2: rgba(255, 255, 255, 0.52);
                --text-primary: #251a14;
                --text-secondary: #6c5a4c;
                --text-tertiary: rgba(37, 26, 20, 0.50);
                --border-light: rgba(37, 26, 20, 0.10);
                --border-medium: rgba(37, 26, 20, 0.16);
                --accent: #8b3f2f; /* rust */
                --accent-hover: #5a2b1f; /* cocoa */
                --accent-light: rgba(139, 63, 47, 0.12);
                --shadow-sm: 0 1px 2px rgba(37, 26, 20, 0.06);
                --shadow-md: 0 10px 30px rgba(37, 26, 20, 0.10);
                --shadow-lg: 0 26px 70px rgba(37, 26, 20, 0.14);
                --radius-sm: 10px;
                --radius-md: 12px;
                --radius-lg: 16px;
            }}
            * {{ box-sizing: border-box; margin: 0; padding: 0; }}
            html {{ height: 100%; }}
            body {{
                min-height: 100%;
                margin: 0;
                font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
                font-size: 15px;
                line-height: 1.6;
                background:
                    radial-gradient(900px 440px at 18% -12%, rgba(139, 63, 47, 0.12), transparent 58%),
                    radial-gradient(900px 440px at 86% -18%, rgba(90, 43, 31, 0.08), transparent 58%),
                    linear-gradient(180deg, var(--bg-primary) 0%, var(--bg-secondary) 100%);
                color: var(--text-primary);
                -webkit-font-smoothing: antialiased;
                -moz-osx-font-smoothing: grayscale;
            }}
            a {{ 
                color: var(--accent); 
                text-decoration: none; 
                transition: color 0.2s ease;
            }}
            a:hover {{ color: var(--accent-hover); }}
            .wrap {{ 
                max-width: 1040px; 
                margin: 0 auto; 
                padding: 26px 16px 68px; 
            }}
            header {{ 
                display: flex; 
                align-items: center; 
                justify-content: space-between; 
                margin-bottom: 32px; 
                padding-bottom: 18px;
                border-bottom: 1px solid var(--border-light);
            }}
            .brand {{ 
                display: flex; 
                align-items: center; 
                gap: 16px; 
            }}
            .mark {{
                width: 48px; 
                height: 48px;
                border-radius: var(--radius-md);
                background: linear-gradient(135deg, rgba(139, 63, 47, 0.92) 0%, rgba(90, 43, 31, 0.92) 100%);
                display: flex; 
                align-items: center; 
                justify-content: center;
                color: white;
                box-shadow: 0 16px 46px rgba(90, 43, 31, 0.20);
                transition: transform 0.16s ease, box-shadow 0.16s ease;
            }}
            .mark:hover {{
                transform: translateY(-2px);
                box-shadow: var(--shadow-lg);
            }}
            .brand h1 {{ 
                font-size: 22px; 
                font-weight: 600; 
                letter-spacing: -0.5px; 
                line-height: 1.2; 
                color: var(--text-primary);
            }}
            .brand p {{ 
                color: var(--text-secondary); 
                font-size: 14px; 
                font-weight: 400;
            }}
            nav {{ 
                display: flex; 
                gap: 8px; 
                align-items: center; 
            }}
            .card {{
                background: var(--surface);
                border: 1px solid var(--border-light);
                border-radius: var(--radius-lg);
                padding: 22px;
                box-shadow: var(--shadow-sm);
                backdrop-filter: blur(10px);
                transition: box-shadow 0.18s ease, transform 0.18s ease, border-color 0.18s ease;
            }}
            ul.results .card:hover {{
                box-shadow: var(--shadow-md);
                transform: translateY(-1px);
                border-color: rgba(37, 26, 20, 0.18);
            }}
            .hero {{ 
                padding: 34px 28px; 
                text-align: center;
                max-width: 920px;
                margin: 0 auto;
            }}
            .hero h2 {{ 
                font-size: 30px; 
                font-weight: 600; 
                letter-spacing: -0.8px; 
                margin-bottom: 12px;
                color: var(--text-primary);
            }}
            .hero p {{ 
                color: var(--text-secondary); 
                font-size: 15px; 
                margin-bottom: 22px;
                max-width: 64ch;
                margin-left: auto;
                margin-right: auto;
            }}
            .searchbar {{ 
                display: flex; 
                gap: 12px; 
                align-items: center;
                width: 100%;
                max-width: 700px;
                margin: 0 auto;
            }}
            .searchbar input {{
                flex: 1 1 auto;
                min-width: 0;
                padding: 12px 14px;
                border-radius: var(--radius-md);
                border: 1px solid var(--border-medium);
                background: rgba(255, 255, 255, 0.90);
                color: var(--text-primary);
                font-size: 15px;
                outline: none;
                transition: border-color 0.16s ease, box-shadow 0.16s ease;
                box-shadow: none;
                height: 44px;
            }}
            .searchbar input:focus {{ 
                border-color: var(--accent); 
                box-shadow: 0 0 0 4px var(--accent-light);
            }}
            .searchbar input::placeholder {{
                color: var(--text-tertiary);
            }}
            .searchbar > .btn,
            .searchbar > button.btn {{
                flex: 0 0 auto;
            }}
            .btn {{
                display: inline-flex;
                align-items: center;
                justify-content: center;
                padding: 10px 14px;
                border-radius: var(--radius-md);
                border: 1px solid var(--border-medium);
                background: rgba(255, 255, 255, 0.92);
                color: var(--text-primary);
                font-size: 14px;
                font-weight: 500;
                cursor: pointer;
                text-decoration: none;
                white-space: nowrap;
                transition: all 0.2s ease;
                box-shadow: none;
                height: 44px;
            }}
            .btn:hover {{ 
                background: rgba(255, 255, 255, 0.98);
                border-color: var(--border-medium);
                box-shadow: 0 1px 2px rgba(17, 24, 39, 0.06);
                transform: translateY(-0.5px);
            }}
            .btn:active {{ 
                transform: translateY(0); 
                box-shadow: none;
            }}
            .btn.primary {{
                background: linear-gradient(135deg, rgba(139, 63, 47, 0.96) 0%, rgba(90, 43, 31, 0.96) 100%);
                border-color: transparent;
                color: white;
                box-shadow: 0 16px 34px rgba(90, 43, 31, 0.18);
            }}
            .btn.primary:hover {{ 
                box-shadow: 0 22px 48px rgba(90, 43, 31, 0.22);
                transform: translateY(-1px);
            }}
            .btn.inline {{ 
                display: inline-flex; 
                gap: 8px; 
            }}
            .btn.icononly {{ 
                padding: 10px;
                min-width: 44px;
            }}
            .icon {{ 
                width: 18px; 
                height: 18px; 
                display: inline-block;
                flex-shrink: 0;
            }}
            ul.results {{ 
                list-style: none; 
                padding: 0; 
                margin: 24px 0 0; 
                display: grid; 
                gap: 16px; 
            }}
            .row {{ 
                display: flex; 
                justify-content: space-between; 
                gap: 20px; 
                align-items: flex-start;
                flex-wrap: wrap;
            }}
            .title {{ 
                font-weight: 600; 
                font-size: 17px;
                margin: 0 0 10px; 
                letter-spacing: -0.3px; 
                color: var(--text-primary);
                line-height: 1.4;
            }}
            .meta {{ 
                color: var(--text-secondary); 
                font-size: 13px;
                line-height: 1.5;
            }}
            .actions {{ 
                display: flex; 
                gap: 8px; 
                align-items: center; 
                flex-wrap: wrap;
            }}
            .pill {{
                display: inline-flex;
                gap: 6px;
                align-items: center;
                padding: 6px 12px;
                border-radius: 999px;
                border: 1px solid var(--border-light);
                background: rgba(255, 255, 255, 0.55);
                font-size: 13px;
                font-weight: 500;
                color: var(--text-secondary);
                white-space: nowrap;
            }}
            code {{
                font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', Consolas, 'Courier New', monospace;
                font-size: 0.92em;
                background: var(--bg-secondary);
                padding: 2px 6px;
                border-radius: 4px;
                color: var(--text-primary);
            }}
            .hash {{ 
                user-select: all; 
                word-break: break-all;
            }}
            footer {{ 
                margin-top: 64px; 
                padding-top: 32px;
                border-top: 1px solid var(--border-light);
                color: var(--text-tertiary); 
                font-size: 13px;
                text-align: center;
            }}
            .empty {{ 
                color: var(--text-secondary); 
                margin: 32px 0; 
                text-align: center;
                font-size: 15px;
            }}
            .two-col {{ 
                display: grid; 
                grid-template-columns: 1fr; 
                gap: 16px; 
            }}
            @media (min-width: 860px) {{
                .two-col {{ 
                    grid-template-columns: 1fr 400px; 
                    align-items: start; 
                }}
            }}
            .field {{ 
                display: flex; 
                gap: 12px; 
                align-items: stretch; 
            }}
            .field input {{ 
                flex: 1; 
            }}
            .toast {{
                position: fixed;
                inset: auto 20px 20px auto;
                background: rgba(37, 26, 20, 0.92);
                color: white;
                border-radius: var(--radius-md);
                padding: 12px 20px;
                font-size: 14px;
                font-weight: 500;
                box-shadow: var(--shadow-lg);
                opacity: 0;
                transform: translateY(12px);
                transition: opacity 0.25s ease, transform 0.25s ease;
                pointer-events: none;
                z-index: 1000;
            }}
            .toast.show {{ 
                opacity: 1; 
                transform: translateY(0); 
            }}
            @media (max-width: 640px) {{
                .wrap {{ padding: 24px 16px 60px; }}
                header {{ margin-bottom: 32px; flex-direction: column; gap: 20px; }}
                .hero {{ padding: 24px; }}
                .hero h2 {{ font-size: 26px; }}
                .searchbar {{ flex-direction: column; align-items: stretch; }}
                .row {{ flex-direction: column; gap: 16px; }}
                .actions {{ width: 100%; justify-content: flex-start; }}
            }}
        </style>
    </head>
    <body>
        <div class="wrap">
            <header>
                <div class="brand">
                    <a class="mark" href="/" aria-label="Home">{}</a>
                    <div>
                        <h1><a href="/">{}</a></h1>
                        <p>{}</p>
                    </div>
                </div>
                <nav>
                    <a class="btn" href="/">Home</a>
                </nav>
            </header>
            {}
            <footer>
                <div>Runs as a single binary. Data persists under <code>data/</code> by default.</div>
            </footer>
        </div>
        <div id="toast" class="toast" role="status" aria-live="polite"></div>
        <script>
            function toast(msg) {{
                const el = document.getElementById('toast');
                if (!el) return;
                el.textContent = msg;
                el.classList.add('show');
                clearTimeout(el._t);
                el._t = setTimeout(() => el.classList.remove('show'), 900);
            }}
            async function copyText(text) {{
                try {{
                    await navigator.clipboard.writeText(text);
                    toast('Copied');
                }} catch (e) {{
                    // Fallback for older browsers
                    const ta = document.createElement('textarea');
                    ta.value = text;
                    ta.style.position = 'fixed';
                    ta.style.opacity = '0';
                    document.body.appendChild(ta);
                    ta.focus();
                    ta.select();
                    try {{ document.execCommand('copy'); toast('Copied'); }} catch (_) {{ toast('Copy failed'); }}
                    document.body.removeChild(ta);
                }}
            }}
            document.addEventListener('click', (ev) => {{
                const btn = ev.target.closest('[data-copy]');
                if (!btn) return;
                ev.preventDefault();
                const text = btn.getAttribute('data-copy') || '';
                if (text.trim().length === 0) return;
                copyText(text);
            }});
        </script>
    </body>
</html>"#,
"##,
        html_escape(&full_title),
        ICON_SEARCH,
        html_escape(APP_TITLE),
        html_escape(APP_TAGLINE),
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

async fn home() -> impl IntoResponse {
    page(
        "Search",
        format!(
            r#"<main class="card hero">
    <h2>Search the index</h2>
    <p>Type a title. Serma continuously discovers hashes, enriches metadata, and ranks by seeders.</p>
    <form action="/search" method="get" class="searchbar" role="search">
        <input name="q" placeholder="Search titles…" autocomplete="off" />
        <button class="btn primary inline" type="submit">{} Search</button>
    </form>
</main>"#,
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

        let magnet_actions_html = if magnet.trim().is_empty() {
            "<span class=\"meta\">No magnet</span>".to_string()
        } else {
            format!(
                "<a class=\"btn primary inline\" rel=\"nofollow\" href=\"{}\" title=\"Open magnet\">{} Magnet</a> <a class=\"btn icononly\" href=\"#\" data-copy=\"{}\" title=\"Copy magnet link\" aria-label=\"Copy magnet link\">{}</a>",
                html_escape(&magnet),
                ICON_MAGNET,
                html_escape(&magnet),
                ICON_COPY
            )
        };
        let details_html = if info_hash.trim().is_empty() {
            "".to_string()
        } else {
            format!(
                "<a class=\"btn\" href=\"/t/{}\">Details</a>",
                html_escape(&info_hash)
            )
        };

        items.push_str(&format!(
                        r#"<li class="card">
    <div class="row">
    <div>
            <div class="title">{}</div>
            <div class="meta">Info hash: <code class="hash">{}</code></div>
    </div>
        <div class="actions">
            <span class="pill">Seeders: {}</span>
      {}{}
    </div>
  </div>
</li>"#,
            html_escape(&title),
            html_escape(&info_hash),
            hit.seeders,
            magnet_actions_html,
            if details_html.is_empty() {
                "".to_string()
            } else {
                format!(" {}", details_html)
            }
        ));
    }

    let results_html = if items.is_empty() {
        "<p class=\"empty\">No results.</p>".to_string()
    } else {
        format!("<ul class=\"results\">{}</ul>", items)
    };

    page(
        &format!("Search: {}", q.trim()),
        format!(
                        r#"<main class="card">
    <form action="/search" method="get" class="searchbar" role="search">
        <input name="q" value="{}" placeholder="Search titles…" />
        <button class="btn primary inline" type="submit">{} Search</button>
  </form>
  {}
</main>"#,
            html_escape(&q),
            ICON_SEARCH,
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
        .unwrap_or_else(|| format!("Item {}", html_escape(&info_hash)));

    let magnet = record
        .as_ref()
        .and_then(|r| r.magnet.clone())
        .unwrap_or_default();
    let magnet_html = if magnet.trim().is_empty() {
        "<span class=\"meta\">No magnet link available yet.</span>".to_string()
    } else {
        format!(
            r##"<div class="two-col">
    <div class="card" style="padding:14px; box-shadow:none; background: rgba(255,255,255,0.45);">
        <div class="meta" style="margin-bottom:6px;">Magnet link</div>
        <div class="field searchbar" style="margin:0;">
            <input value="{}" readonly />
            <a class="btn icononly" href="#" data-copy="{}" title="Copy magnet link" aria-label="Copy magnet link">{}</a>
            <a class="btn primary inline" rel="nofollow" href="{}" title="Open magnet">{} Magnet</a>
        </div>
    </div>
    <div></div>
</div>"##,
            html_escape(&magnet),
            html_escape(&magnet),
            ICON_COPY,
            html_escape(&magnet),
            ICON_MAGNET
        )
    };

    let seeders = record.as_ref().map(|r| r.seeders).unwrap_or(0);
    let has_metadata = record
        .as_ref()
        .and_then(|r| r.info_bencode_base64.as_deref())
        .is_some_and(|s| !s.trim().is_empty());

    page(
        &title,
        format!(
                        r#"<main class="card">
    <div class="row">
    <div>
            <div class="title">{}</div>
            <div class="meta">Info hash: <code class="hash">{}</code></div>
    </div>
        <div class="actions">
            <span class="pill">Seeders: {}</span>
            <span class="pill">Metadata: {}</span>
    </div>
  </div>
  <div style="margin-top: 14px;">{}</div>
</main>"#,
            html_escape(&title),
            html_escape(&info_hash),
            seeders,
            if has_metadata { "yes" } else { "no" },
            magnet_html
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
