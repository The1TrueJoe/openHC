//! Routes + handlers. Serves the embedded SPA and a small REST API over the
//! board's native serial radios/ports, plus a WebSocket serial bridge (the
//! browser xterm talks to a UART through this — replaces ttyd).
use crate::serial::Serial;
use crate::{board::Board, system, Config};
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
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
<h1>ohc-webd</h1><p>The UI was not compiled in. Build it: <code>cd ui &amp;&amp; npm ci &amp;&amp; npm run build</code>, then rebuild.</p>\
<p>API is live: <a style=color:#34d399 href=/api/board>/api/board</a></p>";

pub fn router(cfg: Arc<Config>) -> Router {
    use tower_http::compression::CompressionLayer;
    Router::new()
        .route("/api/board", get(board))
        .route("/api/system", get(sys))
        .route("/api/radios", get(radios))
        .route("/api/radios/{kind}/tx", post(radio_tx))
        .route("/api/radios/{kind}/rx", get(radio_rx))
        .route("/api/radios/{kind}/reset", post(radio_reset))
        .route("/api/serials", get(serials))
        .route("/api/wifi/scan", get(wifi_scan))
        .route("/api/wifi/connect", post(wifi_connect))
        .route("/api/openapi.json", get(openapi))
        .route("/ws/serial/{dev}", get(ws_serial))
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
async fn radios(State(c): State<Arc<Config>>) -> Json<Vec<crate::board::Radio>> {
    Json(load(&c).radios)
}
async fn serials(State(c): State<Arc<Config>>) -> Json<Vec<crate::board::SerialPort>> {
    Json(load(&c).serials)
}

// ── Wi-Fi control (the captive portal is a separate app, ohc-portal; these let
//    the dashboard drive the same scan/join over its API) ──────────────────────
async fn wifi_scan() -> Json<Vec<String>> {
    Json(ohc_wifi::scan_cache())
}

#[derive(serde::Deserialize)]
struct WifiBody {
    ssid: String,
    psk: Option<String>,
}
async fn wifi_connect(State(c): State<Arc<Config>>, Json(b): Json<WifiBody>) -> Response {
    let iface = load(&c).wifi_iface;
    match ohc_wifi::apply(&iface, b.ssid.trim(), b.psk.as_deref().unwrap_or("").trim()) {
        Ok(ssid) => Json(serde_json::json!({ "joining": ssid })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

#[derive(serde::Deserialize)]
struct TxBody {
    hex: Option<String>,
    text: Option<String>,
}

async fn radio_tx(
    State(c): State<Arc<Config>>,
    Path(kind): Path<String>,
    Json(b): Json<TxBody>,
) -> Response {
    let Some(dev) = load(&c).radio_dev(&kind) else {
        return err(StatusCode::NOT_FOUND, "no such radio");
    };
    let data = if let Some(h) = b.hex {
        match hex_decode(&h) {
            Ok(d) => d,
            Err(_) => return err(StatusCode::BAD_REQUEST, "bad hex"),
        }
    } else {
        b.text.unwrap_or_default().into_bytes()
    };
    match Serial::open(&dev, 115200).and_then(|mut s| s.write_all(&data)) {
        Ok(n) => Json(serde_json::json!({ "written": n })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct RxQ {
    ms: Option<u64>,
}
async fn radio_rx(
    State(c): State<Arc<Config>>,
    Path(kind): Path<String>,
    Query(q): Query<RxQ>,
) -> Response {
    let Some(dev) = load(&c).radio_dev(&kind) else {
        return err(StatusCode::NOT_FOUND, "no such radio");
    };
    let ms = q.ms.unwrap_or(500).min(5000);
    let data = tokio::task::spawn_blocking(move || {
        Serial::open(&dev, 115200).map(|mut s| s.read_for(ms)).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Json(serde_json::json!({ "hex": hex_encode(&data), "text": String::from_utf8_lossy(&data) }))
        .into_response()
}

async fn radio_reset(Path(kind): Path<String>) -> Response {
    // Pulse the <kind>_reset gpio line by name via libgpiod tools.
    let line = format!("{kind}_reset");
    let ok = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("gpioset -m time -u 200000 $(gpiofind {line})"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    Json(serde_json::json!({ "reset": ok })).into_response()
}

// ── WebSocket serial bridge ──────────────────────────────────────────────────
async fn ws_serial(
    State(c): State<Arc<Config>>,
    Path(dev): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let dev = format!("/dev/{dev}");
    let baud = load(&c).serial_baud(&dev);
    // guard: only ports the board actually declares
    if !load(&c).serials.iter().any(|s| s.dev == dev) {
        return err(StatusCode::NOT_FOUND, "no such serial");
    }
    ws.on_upgrade(move |sock| serial_bridge(sock, dev, baud))
}

async fn serial_bridge(mut sock: WebSocket, dev: String, baud: u32) {
    use futures_util::SinkExt;
    use futures_util::StreamExt;
    let Ok(serial) = Serial::open(&dev, baud) else {
        let _ = sock.send(Message::Text(format!("cannot open {dev}").into())).await;
        return;
    };
    // Blocking task owns the port: drains to_serial writes and streams reads out.
    let (to_serial_tx, mut to_serial_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (from_serial_tx, mut from_serial_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || {
        let mut serial = serial;
        let mut buf = [0u8; 1024];
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            while let Ok(w) = to_serial_rx.try_recv() {
                let _ = serial.write_all(&w);
            }
            match serial.try_read(&mut buf) {
                Ok(n) if n > 0 => {
                    if from_serial_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
    });

    let (mut ws_tx, mut ws_rx) = sock.split();
    loop {
        tokio::select! {
            Some(chunk) = from_serial_rx.recv() => {
                if ws_tx.send(Message::Binary(chunk.into())).await.is_err() { break; }
            }
            msg = ws_rx.next() => {
                match msg {
                    // Only binary is written to the UART; text control frames are dropped.
                    Some(Ok(Message::Binary(b))) => { let _ = to_serial_tx.send(b.to_vec()); }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = handle.join();
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

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ())).collect()
}
