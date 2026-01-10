use crate::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Deserialize;

const APP_TITLE: &str = "Serma";
const APP_TAGLINE: &str = "Distributed torrent indexing (MVP)";

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
        <meta name="color-scheme" content="light dark" />
        <title>{}</title>
        <style>
            :root {{
                --bg: Canvas;
                --fg: CanvasText;
                --muted: color-mix(in oklab, CanvasText 65%, Canvas 35%);
                --card: color-mix(in oklab, Canvas 92%, CanvasText 8%);
                --border: color-mix(in oklab, CanvasText 20%, Canvas 80%);
                --link: LinkText;
                --radius: 12px;
            }}
            * {{ box-sizing: border-box; }}
            html, body {{ height: 100%; }}
            body {{
                margin: 0;
                font: 16px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Ubuntu, Cantarell, Noto Sans, Arial;
                background: var(--bg);
                color: var(--fg);
            }}
            a {{ color: var(--link); text-decoration: none; }}
            a:hover {{ text-decoration: underline; }}
            .wrap {{ max-width: 980px; margin: 0 auto; padding: 24px 16px 48px; }}
            header {{ display:flex; gap: 16px; align-items: baseline; justify-content: space-between; margin-bottom: 18px; }}
            .brand {{ display:flex; flex-direction: column; gap: 2px; }}
            .brand h1 {{ margin: 0; font-size: 22px; letter-spacing: 0.2px; }}
            .brand p {{ margin: 0; color: var(--muted); font-size: 13px; }}
            nav {{ display:flex; gap: 12px; align-items: center; }}
            .card {{
                background: var(--card);
                border: 1px solid var(--border);
                border-radius: var(--radius);
                padding: 16px;
            }}
            .searchbar {{ display:flex; gap: 10px; align-items: center; }}
            .searchbar input {{
                flex: 1;
                padding: 10px 12px;
                border-radius: 10px;
                border: 1px solid var(--border);
                background: color-mix(in oklab, var(--bg) 92%, var(--fg) 8%);
                color: var(--fg);
                outline: none;
            }}
            .searchbar input:focus {{ border-color: color-mix(in oklab, var(--link) 50%, var(--border) 50%); }}
            .btn {{
                display:inline-block;
                padding: 10px 12px;
                border-radius: 10px;
                border: 1px solid var(--border);
                background: color-mix(in oklab, var(--bg) 86%, var(--fg) 14%);
                color: var(--fg);
                cursor: pointer;
                text-decoration: none;
                white-space: nowrap;
            }}
            .btn:hover {{ background: color-mix(in oklab, var(--bg) 82%, var(--fg) 18%); text-decoration: none; }}
            ul.results {{ list-style: none; padding: 0; margin: 14px 0 0; display: grid; gap: 10px; }}
            .row {{ display:flex; justify-content: space-between; gap: 14px; align-items: flex-start; }}
            .title {{ font-weight: 650; margin: 0 0 4px; }}
            .meta {{ color: var(--muted); font-size: 13px; }}
            .actions {{ display:flex; gap: 10px; align-items: center; flex-wrap: wrap; }}
            .pill {{
                display:inline-flex;
                gap: 6px;
                align-items: center;
                padding: 6px 10px;
                border-radius: 999px;
                border: 1px solid var(--border);
                background: color-mix(in oklab, var(--bg) 90%, var(--fg) 10%);
                font-size: 13px;
                color: var(--muted);
            }}
            code {{
                font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, Liberation Mono, monospace;
                font-size: 0.95em;
            }}
            footer {{ margin-top: 22px; color: var(--muted); font-size: 12px; }}
            .empty {{ color: var(--muted); margin: 12px 0 0; }}
        </style>
    </head>
    <body>
        <div class="wrap">
            <header>
                <div class="brand">
                    <h1><a href="/">{}</a></h1>
                    <p>{}</p>
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
    </body>
</html>"#,
        html_escape(&full_title),
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
            r#"<main class="card">
    <div class="row">
        <div>
            <p class="meta">Search torrents by name. Results are ranked by seeders.</p>
        </div>
    </div>
    <form action="/search" method="get" class="searchbar">
        <input name="q" placeholder="Search titles…" autocomplete="off" />
        <button class="btn" type="submit">Search</button>
    </form>
</main>"#
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

        let download_html = if magnet.trim().is_empty() {
            "<span class=\"meta\">No download link</span>".to_string()
        } else {
            format!(
                "<a class=\"btn\" rel=\"nofollow\" href=\"{}\">Download</a>",
                html_escape(&magnet)
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
            <div class="meta">Info hash: <code>{}</code></div>
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
            download_html,
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
    <form action="/search" method="get" class="searchbar">
        <input name="q" value="{}" placeholder="Search titles…" />
        <button class="btn" type="submit">Search</button>
  </form>
  {}
</main>"#,
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
        .unwrap_or_else(|| format!("Item {}", html_escape(&info_hash)));

    let magnet = record
        .as_ref()
        .and_then(|r| r.magnet.clone())
        .unwrap_or_default();
    let magnet_html = if magnet.trim().is_empty() {
        "<span class=\"meta\">No download link</span>".to_string()
    } else {
        format!(
            "<a class=\"btn\" rel=\"nofollow\" href=\"{}\">Download</a>",
            html_escape(&magnet)
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
            <div class="meta">Info hash: <code>{}</code></div>
    </div>
        <div class="actions">
            <span class="pill">Seeders: {}</span>
            <span class="pill">Metadata: {}</span>
      {}
    </div>
  </div>
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
