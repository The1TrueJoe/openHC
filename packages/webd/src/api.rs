//! Routes + handlers. Serves the embedded SPA and a small REST API over the
//! board's native serial radios/ports, plus a WebSocket serial bridge (the
//! browser xterm talks to a UART through this — replaces ttyd).
use crate::{board::Board, system, Config};
use axum::{
    body::Body,
    extract::{
        State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use std::sync::Arc;

pub struct Asset {
    pub path: &'static str,
    pub mime: &'static str,
    pub etag: &'static str,
    pub raw: &'static [u8],
    pub gzip: Option<&'static [u8]>,
}
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

const FALLBACK: &str = "<!doctype html><meta charset=utf-8><title>openHC</title>\
<body style=\"font:15px system-ui;background:#0c111d;color:#e6e9ef;padding:2rem\">\
<h1>webd</h1><p>The UI was not compiled in. Build it: <code>cd ui &amp;&amp; npm ci &amp;&amp; npm run build</code>, then rebuild.</p>\
<p>API is live: <a style=color:#34d399 href=/api/board>/api/board</a></p>";

pub fn router(cfg: Arc<Config>) -> Router {
    use tower_http::compression::CompressionLayer;
    Router::new()
        .route("/api/board", get(board))
        .route("/api/system", get(sys))
        .route("/api/wifi/scan", get(wifi_scan))
        .route("/api/wifi/connect", post(wifi_connect))
        .route("/api/openapi.json", get(openapi))
        // Everything the IO server owns, on this origin. See proxy.rs for why
        // the GUI must not be asked to reach a second port itself.
        .route("/iod/{*rest}", any(crate::proxy::handler))
        .fallback(fallback)
        .layer(CompressionLayer::new())
        .with_state(cfg)
}

fn load(cfg: &Config) -> Board {
    Board::load(&cfg.board_env)
}

async fn board(State(c): State<Arc<Config>>) -> Json<Board> {
    Json(load(&c))
}
async fn sys() -> Json<system::System> {
    Json(system::snapshot())
}
// ── Wi-Fi control (the captive portal is a separate app, portal; these let
//    the dashboard drive the same scan/join over its API) ──────────────────────
async fn wifi_scan() -> Json<Vec<String>> {
    Json(wifi::scan_cache())
}

#[derive(serde::Deserialize)]
struct WifiBody {
    ssid: String,
    psk: Option<String>,
}
async fn wifi_connect(State(c): State<Arc<Config>>, Json(b): Json<WifiBody>) -> Response {
    let iface = load(&c).wifi_iface;
    match wifi::apply(&iface, b.ssid.trim(), b.psk.as_deref().unwrap_or("").trim()) {
        Ok(ssid) => Json(serde_json::json!({ "joining": ssid })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

// ── fallback: bounce to the setup portal while the AP is up, else the SPA ────
async fn fallback(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    // S41wifi-ap drops the setup portal's URL here while the Wi-Fi AP is up. The
    // portal is a separate app on its own port; we just redirect the phone's OS
    // connectivity check (and any other request) to it — that 302 is what pops
    // the captive portal. Empty/absent file → normal dashboard serving.
    if let Ok(url) = std::fs::read_to_string("/tmp/ohc-ap-portal") {
        let url = url.trim();
        if !url.is_empty() {
            return Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, url)
                .body(Body::empty())
                .unwrap();
        }
    }
    static_asset(headers, uri).await
}

// ── static assets (SPA) ──────────────────────────────────────────────────────
async fn static_asset(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }
    let asset = EMBEDDED_ASSETS
        .iter()
        .find(|a| a.path == path)
        // SPA fallback: unknown paths serve index.html
        .or_else(|| EMBEDDED_ASSETS.iter().find(|a| a.path == "index.html"));

    let Some(a) = asset else {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(FALLBACK))
            .unwrap();
    };
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(a.etag) {
        return Response::builder().status(StatusCode::NOT_MODIFIED).body(Body::empty()).unwrap();
    }
    let accept_gzip = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("gzip"))
        .unwrap_or(false);
    let mut b = Response::builder()
        .header(header::CONTENT_TYPE, a.mime)
        .header(header::ETAG, a.etag);
    if a.path.starts_with("assets/") {
        b = b.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
    }
    match (accept_gzip, a.gzip) {
        (true, Some(gz)) => b
            .header(header::CONTENT_ENCODING, "gzip")
            .body(Body::from(gz))
            .unwrap(),
        _ => b.body(Body::from(a.raw)).unwrap(),
    }
}

async fn openapi() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("openapi.json"),
    )
        .into_response()
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(serde_json::json!({ "error": msg }))).into_response()
}

