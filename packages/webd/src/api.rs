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
    routing::{any, get},
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

/// webd's own generated spec, captured when the router is built so the
/// /api/openapi.json handler can merge the other daemons' documents into it.
static OWN_SPEC: std::sync::OnceLock<utoipa::openapi::OpenApi> = std::sync::OnceLock::new();

/// Document metadata. Paths come from the handlers; the other daemons' paths
/// are merged in at request time by `openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "openHC",
        description = "REST across the openHC daemons as the browser sees them: webd serves the \
page on :80 and reverse-proxies the others, so everything here is one origin.\n\n\
IO **control** is not here — it is MQTT, because a relay closing is state an automation subscribes \
to rather than a request/response. See the AsyncAPI document.",
    ),
    tags(
        (name = "Board", description = "Identity, capabilities and runtime state of this unit."),
        (name = "Wi-Fi", description = "Setup-AP scanning and joining."),
        (name = "Display", description = "The opt-in on-panel web view (off by default; the panel \
shows the splash until it is enabled here)."),
        (name = "Reference", description = "The specifications this box serves."),
    )
)]
pub struct ApiDoc;

pub fn router(cfg: Arc<Config>) -> Router {
    use tower_http::compression::CompressionLayer;
    let (rest, api) = utoipa_axum::router::OpenApiRouter::with_openapi(
        <ApiDoc as utoipa::OpenApi>::openapi(),
    )
    .routes(utoipa_axum::routes!(board))
    .routes(utoipa_axum::routes!(sys))
    .routes(utoipa_axum::routes!(wifi_scan))
    .routes(utoipa_axum::routes!(wifi_connect))
    .routes(utoipa_axum::routes!(display_get, display_set))
    .routes(utoipa_axum::routes!(display_screenshot))
    .split_for_parts();
    OWN_SPEC.set(api).ok();

    // Behind the controller's token when one is set (auth.rs): webd's own API
    // and everything proxied to a daemon that listens on loopback only.
    let protected = Router::new()
        .merge(rest)
        // Telemetry: sysmond, its own daemon and its own port.
        .route("/sys/{*rest}", any(crate::proxy::handler))
        .route("/audio/{*rest}", any(crate::proxy::handler))
        .route("/storage/{*rest}", any(crate::proxy::handler))
        // Managed-switch config: switchd, present only on switch boards. Absent
        // elsewhere, the proxy just returns a tidy 502 and the UI shows no panel.
        .route("/switch/{*rest}", any(crate::proxy::handler))
        // IO control is MQTT, and the browser speaks it here. Same origin as
        // the page, so one open port is enough for the whole GUI.
        .route("/mqtt", any(crate::proxy::handler))
        .route_layer(axum::middleware::from_fn(crate::auth::require));

    Router::new()
        .merge(protected)
        .route("/api/openapi.json", get(openapi))
        .route("/api/asyncapi.json", get(asyncapi))
        // Everything the IO server owns, on this origin. See proxy.rs for why
        // the GUI must not be asked to reach a second port itself. iod checks
        // the token itself.
        .route("/iod/{*rest}", any(crate::proxy::handler))
        .fallback(fallback)
        .layer(CompressionLayer::new())
        .with_state(cfg)
}

fn load(cfg: &Config) -> Board {
    Board::load(&cfg.board_env)
}

#[utoipa::path(get, path = "/api/board", tag = "Board",
    summary = "Board identity and capabilities",
    description = "Read from /opt/ohc/board.env — the one file that differs between an HC-800, an \
EA3, an IO Extender and a CA-1.",
    responses((status = 200, description = "board")))]
async fn board(State(c): State<Arc<Config>>) -> Json<Board> {
    Json(load(&c))
}
#[utoipa::path(get, path = "/api/system", tag = "Board",
    summary = "Runtime state: uptime, kernel, interfaces",
    responses((status = 200, description = "system")))]
async fn sys() -> Json<system::System> {
    Json(system::snapshot())
}
// ── Wi-Fi control (the captive portal is a separate app, portal; these let
//    the dashboard drive the same scan/join over its API) ──────────────────────
#[utoipa::path(get, path = "/api/wifi/scan", tag = "Wi-Fi",
    summary = "Nearby networks",
    description = "Only meaningful on a board with a radio; boards without one report none rather \
than failing.",
    responses((status = 200, description = "SSIDs")))]
async fn wifi_scan() -> Json<Vec<String>> {
    Json(wifi::scan_cache())
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct WifiBody {
    ssid: String,
    psk: Option<String>,
}
#[utoipa::path(post, path = "/api/wifi/connect", tag = "Wi-Fi",
    summary = "Join a network",
    description = "The captive-portal path: used from the setup AP a board raises when it has no \
wired carrier.",
    responses((status = 200, description = "joined")))]
async fn wifi_connect(State(c): State<Arc<Config>>, Json(b): Json<WifiBody>) -> Response {
    let iface = load(&c).wifi_iface;
    match wifi::apply(&iface, b.ssid.trim(), b.psk.as_deref().unwrap_or("").trim()) {
        Ok(ssid) => Json(serde_json::json!({ "joining": ssid })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

// ── On-panel web view (opt-in) ───────────────────────────────────────────────
// The panel shows the openHC splash by default; this turns the on-screen browser
// on and points it somewhere. State lives in /opt/ohc/display.conf, which the
// S95ohc-display init script sources — see that script for the launch itself.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
struct Display {
    /// Show the on-panel web view. Off by default: a fresh box shows the splash.
    enabled: bool,
    /// Page the view loads; defaults to this controller's own dashboard.
    url: String,
    /// Day/night for the on-panel view: "auto" (follow the page's own preference),
    /// "light" or "dark". The panel has no OS light/dark signal, so this forces it;
    /// passed to the dashboard as ?theme= (the UI keys off it).
    #[serde(default = "default_theme")]
    theme: String,
}
fn default_theme() -> String { "auto".into() }
impl Default for Display {
    fn default() -> Self {
        Self { enabled: false, url: "http://localhost/".into(), theme: default_theme() }
    }
}

/// display.conf sits next to board.env (that path is overridable for tests).
fn display_conf(cfg: &Config) -> std::path::PathBuf {
    std::path::Path::new(&cfg.board_env)
        .parent().unwrap_or_else(|| std::path::Path::new("/opt/ohc"))
        .join("display.conf")
}

fn read_display(cfg: &Config) -> Display {
    let mut d = Display::default();
    if let Ok(s) = std::fs::read_to_string(display_conf(cfg)) {
        for line in s.lines() {
            let Some((k, v)) = line.trim().split_once('=') else { continue };
            let v = v.trim().trim_matches('"').trim_matches('\'');
            match k.trim() {
                "OHC_DISPLAY_ENABLE" => d.enabled = v == "1",
                "OHC_DISPLAY_URL" if !v.is_empty() => d.url = v.to_string(),
                "OHC_DISPLAY_THEME" if !v.is_empty() => d.theme = v.to_string(),
                _ => {}
            }
        }
    }
    d
}

#[utoipa::path(get, path = "/api/display", tag = "Display",
    summary = "On-panel web-view state",
    description = "Reads /opt/ohc/display.conf. Off by default — a fresh box shows the splash.",
    responses((status = 200, description = "display config")))]
async fn display_get(State(c): State<Arc<Config>>) -> Json<Display> {
    Json(read_display(&c))
}

#[utoipa::path(post, path = "/api/display", tag = "Display",
    summary = "Turn the on-panel web view on/off and set its URL",
    description = "Writes /opt/ohc/display.conf and restarts the display service to apply it now \
(EA only; a no-op on boards without the on-panel WPE stack). The config persists, so the choice \
survives a reboot.",
    responses((status = 200, description = "applied")))]
async fn display_set(State(c): State<Arc<Config>>, Json(d): Json<Display>) -> Response {
    let url = d.url.trim();
    if d.enabled && !(url.starts_with("http://") || url.starts_with("https://")) {
        return err(StatusCode::BAD_REQUEST, "url must start with http:// or https://");
    }
    let url = if url.is_empty() { "http://localhost/" } else { url };
    let theme = match d.theme.trim() {
        "light" => "light", "dark" => "dark", _ => "auto",
    };
    let body = format!(
        "# written by webd POST /api/display — the on-panel web view\n\
         OHC_DISPLAY_ENABLE={}\nOHC_DISPLAY_URL={}\nOHC_DISPLAY_THEME={}\n",
        if d.enabled { 1 } else { 0 }, url, theme);
    let path = display_conf(&c);
    if let Err(e) = std::fs::write(&path, body) {
        return err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("cannot write {}: {e}", path.display()));
    }
    // Apply now by restarting the display service — fire-and-forget (it backgrounds
    // the view). Absent on non-EA boards; the persisted config still takes effect
    // on the next boot, so a missing service is not an error.
    let svc = "/etc/init.d/S95ohc-display";
    let applied = std::path::Path::new(svc).exists()
        && std::process::Command::new(svc).arg("restart").spawn().is_ok();
    Json(serde_json::json!({ "enabled": d.enabled, "url": url, "theme": theme, "applied": applied }))
        .into_response()
}

// ── Framebuffer screenshot ───────────────────────────────────────────────────
// What is ACTUALLY on the panel right now, as an image — the on-panel view renders
// straight to /dev/fb0 with no window system to ask, so a grab of the framebuffer
// is the only way to see it remotely (and to tell "rendering blank" from "not
// rendering"). BMP, deliberately: no compression, no new crate, and every browser
// shows it. The panel is small (720x480) so the ~1.4 MB is fine for the occasional
// look this is for.
fn capture_fb() -> Result<Vec<u8>, String> {
    let rd = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let num = |s: &str| s.trim().parse::<usize>().map_err(|e| format!("{s:?}: {e}"));
    // Geometry from sysfs — no ioctl, no fb_var_screeninfo struct to get wrong.
    let vs = rd("/sys/class/graphics/fb0/virtual_size")?;
    let (ws, hs) = vs.trim().split_once(',').ok_or("virtual_size has no comma")?;
    let (w, h) = (num(ws)?, num(hs)?);
    let bpp = num(&rd("/sys/class/graphics/fb0/bits_per_pixel")?)?;
    if bpp != 32 {
        return Err(format!("only 32bpp is handled, panel is {bpp}bpp"));
    }
    // stride (line_length) is usually padded past width*4 (the EA panel is 2944 for
    // 720 px), so rows must be copied by stride, not width.
    let stride = rd("/sys/class/graphics/fb0/stride").ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(w * 4);
    let fb = std::fs::read("/dev/fb0").map_err(|e| format!("/dev/fb0: {e}"))?;
    let row = w * 4;
    let img = row * h;
    let mut out = Vec::with_capacity(54 + img);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + img) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER — 32bpp BI_RGB, positive height = bottom-up rows.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(img as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // Pixels bottom-up; fb0 is BGRA, which is exactly BMP 32bpp byte order.
    for y in (0..h).rev() {
        let s = y * stride;
        out.extend_from_slice(fb.get(s..s + row).ok_or("framebuffer shorter than its geometry")?);
    }
    Ok(out)
}

#[utoipa::path(get, path = "/api/display/screenshot", tag = "Display",
    summary = "Grab the controller's panel as an image",
    description = "A BMP of /dev/fb0 — what is on the on-panel screen right now (splash, the web \
view, whatever). Boards with no framebuffer return an error. Handy for a remote look without \
standing in front of the unit.",
    responses((status = 200, description = "image/bmp", content_type = "image/bmp")))]
async fn display_screenshot() -> Response {
    match capture_fb() {
        Ok(bmp) => (
            [(header::CONTENT_TYPE, "image/bmp"),
             (header::CACHE_CONTROL, "no-store")],
            bmp,
        ).into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, &format!("no framebuffer grab: {e}")),
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

/// The MQTT surface. Served next to the OpenAPI document because a reader
/// wanting "what can this box do" should not have to know which of the two
/// protocols a given capability lives on.
async fn asyncapi() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        include_str!("asyncapi.json"),
    )
        .into_response()
}

/// The API reference, ASSEMBLED FROM THE RUNNING SYSTEM.
///
/// webd's own paths come from its router (see ApiDoc); iod's and sysmond's are
/// fetched from those daemons at request time and merged under the mount points
/// the browser reaches them on. Nothing here is hand-maintained, which is the
/// point: the previous static openapi.json still described /api/radios and
/// /api/serials months after both were removed, and nobody noticed because
/// nothing checked.
///
/// A daemon that is down contributes nothing rather than failing the document —
/// its absence from the reference is then a true statement about the box.
async fn openapi() -> Response {
    let mut doc = match OWN_SPEC.get() {
        Some(a) => serde_json::to_value(a).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };
    for (addr, mount, group) in [
        (crate::proxy::iod_addr(), "/iod", "iod"),
        (crate::proxy::sysmond_addr(), "/sys", "sysmond"),
        (crate::proxy::audiod_addr(), "/audio", "ohc-audiod"),
        (crate::proxy::storaged_addr(), "/storage", "ohc-storaged"),
        (crate::proxy::switchd_addr(), "/switch", "switchd"),
    ] {
        if let Some(sub) = fetch_spec(&addr).await {
            merge_spec(&mut doc, sub, mount, group);
        }
    }
    ([(axum::http::header::CONTENT_TYPE, "application/json")], Json(doc)).into_response()
}

/// GET /api/openapi.json from a loopback daemon. Deliberately tolerant: any
/// failure means that daemon simply is not described.
async fn fetch_spec(addr: &str) -> Option<serde_json::Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::time::timeout(
        std::time::Duration::from_millis(800),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .ok()?
    .ok()?;
    let req = format!("GET /api/openapi.json HTTP/1.0\r\nHost: {addr}\r\n\r\n");
    s.write_all(req.as_bytes()).await.ok()?;
    let mut buf = Vec::new();
    tokio::time::timeout(std::time::Duration::from_millis(1500), s.read_to_end(&mut buf))
        .await
        .ok()?
        .ok()?;
    let body_at = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    serde_json::from_slice(&buf[body_at..]).ok()
}

/// Fold `sub` into `doc`, moving every path under `mount` and prefixing every
/// tag with `group` so the sidebar groups by daemon rather than interleaving
/// three unrelated services.
fn merge_spec(doc: &mut serde_json::Value, sub: serde_json::Value, mount: &str, group: &str) {
    let Some(paths) = sub.get("paths").and_then(|p| p.as_object()) else {
        return;
    };
    let tag_of = |t: &str| format!("{group}: {t}");

    // Carry the sub-document's tag descriptions across, renamed to match.
    if let Some(tags) = sub.get("tags").and_then(|t| t.as_array()) {
        let dst = doc
            .as_object_mut()
            .unwrap()
            .entry("tags")
            .or_insert_with(|| serde_json::json!([]));
        if let Some(arr) = dst.as_array_mut() {
            for t in tags {
                let mut t = t.clone();
                if let Some(n) = t.get("name").and_then(|n| n.as_str()).map(|n| tag_of(n)) {
                    t["name"] = serde_json::json!(n);
                    arr.push(t);
                }
            }
        }
    }

    let dst = doc
        .as_object_mut()
        .unwrap()
        .entry("paths")
        .or_insert_with(|| serde_json::json!({}));
    let Some(dst) = dst.as_object_mut() else { return };
    for (path, item) in paths {
        let mut item = item.clone();
        if let Some(ops) = item.as_object_mut() {
            for (_, op) in ops.iter_mut() {
                if let Some(tags) = op.get_mut("tags").and_then(|t| t.as_array_mut()) {
                    for t in tags.iter_mut() {
                        if let Some(n) = t.as_str().map(|n| tag_of(n)) {
                            *t = serde_json::json!(n);
                        }
                    }
                }
            }
        }
        dst.insert(format!("{mount}{path}"), item);
    }
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(serde_json::json!({ "error": msg }))).into_response()
}

