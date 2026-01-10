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

const ICON_MAGNET: &str = r#"<svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\" aria-hidden=\"true\"><path d=\"M7 3a2 2 0 0 0-2 2v7a7 7 0 0 0 14 0V5a2 2 0 0 0-2-2h-2v9a3 3 0 0 1-6 0V3H7Z\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/><path d=\"M9 3v9a3 3 0 0 0 6 0V3\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"0.55\"/></svg>"#;
const ICON_COPY: &str = r#"<svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\" aria-hidden=\"true\"><path d=\"M9 9h10v11H9V9Z\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linejoin=\"round\"/><path d=\"M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\"/></svg>"#;
const ICON_SEARCH: &str = r#"<svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\" aria-hidden=\"true\"><path d=\"M10.5 18a7.5 7.5 0 1 1 0-15 7.5 7.5 0 0 1 0 15Z\" stroke=\"currentColor\" stroke-width=\"1.7\"/><path d=\"M21 21l-4.2-4.2\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\"/></svg>"#;

fn page(title: &str, body: String) -> Html<String> {
    let full_title = if title.trim().is_empty() {
        APP_TITLE.to_string()
    } else {
        format!("{} · {}", title, APP_TITLE)
    };

    Html(format!(
        r#"<!doctype html>
<html lang="en">
    <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="color-scheme" content="light" />
        <title>{}</title>
        <style>
            :root {{
                /* Rust/chocolate palette (light, minimal) */
                --paper: #fbf6ef;
                --paper-2: #f3ece3;
                --ink: #2a2018;
                --muted: #6f5b4b;
                --border: rgba(42, 32, 24, 0.14);
                --card: rgba(255, 255, 255, 0.6);
                --accent: #8b3f2f; /* rust */
                --accent-2: #5a2b1f; /* cocoa */
                --radius: 14px;
                --shadow: 0 10px 30px rgba(42, 32, 24, 0.08);
            }}
            * {{ box-sizing: border-box; }}
            html, body {{ height: 100%; }}
            body {{
                margin: 0;
                font: 16px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Ubuntu, Cantarell, Noto Sans, Arial;
                background: radial-gradient(1000px 420px at 30% -10%, rgba(139, 63, 47, 0.10), transparent 60%),
                            radial-gradient(900px 340px at 80% 0%, rgba(90, 43, 31, 0.08), transparent 55%),
                            linear-gradient(180deg, var(--paper), var(--paper-2));
                color: var(--ink);
            }}
            a {{ color: var(--accent-2); text-decoration: none; }}
            a:hover {{ text-decoration: underline; }}
            .wrap {{ max-width: 1040px; margin: 0 auto; padding: 28px 16px 56px; }}
            header {{ display:flex; gap: 16px; align-items: center; justify-content: space-between; margin-bottom: 18px; }}
            .brand {{ display:flex; align-items: center; gap: 12px; }}
            .mark {{
                width: 40px; height: 40px;
                border-radius: 12px;
                background: linear-gradient(135deg, rgba(139, 63, 47, 0.18), rgba(90, 43, 31, 0.10));
                border: 1px solid var(--border);
                box-shadow: var(--shadow);
                display:flex; align-items:center; justify-content:center;
                color: var(--accent-2);
            }}
            .brand h1 {{ margin: 0; font-size: 18px; letter-spacing: 0.2px; line-height: 1.1; }}
            .brand p {{ margin: 0; color: var(--muted); font-size: 13px; }}
            nav {{ display:flex; gap: 10px; align-items: center; }}
            .card {{
                background: var(--card);
                border: 1px solid var(--border);
                border-radius: var(--radius);
                padding: 18px;
                box-shadow: var(--shadow);
                backdrop-filter: blur(10px);
            }}
            .hero {{ padding: 22px; }}
            .hero h2 {{ margin: 0 0 6px; font-size: 22px; letter-spacing: -0.2px; }}
            .hero p {{ margin: 0 0 14px; color: var(--muted); font-size: 14px; max-width: 62ch; }}
            .searchbar {{ display:flex; gap: 10px; align-items: center; }}
            .searchbar input {{
                flex: 1;
                padding: 12px 12px;
                border-radius: 12px;
                border: 1px solid var(--border);
                background: rgba(255,255,255,0.55);
                color: var(--ink);
                outline: none;
            }}
            .searchbar input:focus {{ border-color: rgba(139, 63, 47, 0.35); box-shadow: 0 0 0 4px rgba(139, 63, 47, 0.10); }}
            .btn {{
                display:inline-block;
                padding: 10px 12px;
                border-radius: 12px;
                border: 1px solid var(--border);
                background: rgba(255,255,255,0.55);
                color: var(--ink);
                cursor: pointer;
                text-decoration: none;
                white-space: nowrap;
            }}
            .btn:hover {{ background: rgba(255,255,255,0.70); text-decoration: none; }}
            .btn:active {{ transform: translateY(1px); }}
            .btn.primary {{
                background: linear-gradient(180deg, rgba(139, 63, 47, 0.95), rgba(90, 43, 31, 0.95));
                border-color: rgba(90, 43, 31, 0.30);
                color: #fff;
            }}
            .btn.primary:hover {{ background: linear-gradient(180deg, rgba(139, 63, 47, 1.0), rgba(90, 43, 31, 1.0)); }}
            .btn.inline {{ display:inline-flex; align-items: center; gap: 8px; }}
            .btn.icononly {{ padding: 10px; }}
            .icon {{ width: 16px; height: 16px; display:inline-block; }}
            ul.results {{ list-style: none; padding: 0; margin: 14px 0 0; display: grid; gap: 12px; }}
            .row {{ display:flex; justify-content: space-between; gap: 14px; align-items: flex-start; }}
            .title {{ font-weight: 680; margin: 0 0 6px; letter-spacing: -0.15px; }}
            .meta {{ color: var(--muted); font-size: 13px; }}
            .actions {{ display:flex; gap: 10px; align-items: center; flex-wrap: wrap; justify-content: flex-end; }}
            .pill {{
                display:inline-flex;
                gap: 6px;
                align-items: center;
                padding: 6px 10px;
                border-radius: 999px;
                border: 1px solid var(--border);
                background: rgba(255,255,255,0.45);
                font-size: 13px;
                color: var(--muted);
            }}
            code {{
                font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace;
                font-size: 0.95em;
            }}
            .hash {{ user-select: all; }}
            footer {{ margin-top: 22px; color: var(--muted); font-size: 12px; }}
            .empty {{ color: var(--muted); margin: 12px 0 0; }}
            .two-col {{ display:grid; grid-template-columns: 1fr; gap: 12px; }}
            @media (min-width: 860px) {{
                .two-col {{ grid-template-columns: 1fr 360px; align-items: start; }}
            }}
            .field {{ display:flex; gap: 10px; align-items: center; }}
            .field input {{ flex:1; }}
            .toast {{
                position: fixed;
                inset: auto 16px 16px auto;
                background: rgba(42, 32, 24, 0.92);
                color: #fff;
                border-radius: 12px;
                padding: 10px 12px;
                font-size: 13px;
                opacity: 0;
                transform: translateY(6px);
                transition: opacity 160ms ease, transform 160ms ease;
                pointer-events: none;
            }}
            .toast.show {{ opacity: 1; transform: translateY(0); }}
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
