//! The HTTP surfaces — deliberately NOT where IO control lives.
//!
//! IO control is MQTT, and only MQTT: `/mqtt` here is an MQTT-over-WebSocket
//! endpoint for the browser, and it speaks exactly the topics an external
//! broker sees. One set of semantics for the config GUI, Home Assistant, a
//! Node-RED flow and a shell script — rather than a bespoke JSON protocol that
//! has to be kept in step with the MQTT one forever.
//!
//! What is left over HTTP is what MQTT is bad at:
//!
//! * **what the board IS** — `/api/io` capabilities. A client needs this before
//!   it knows which topics exist, so it cannot itself arrive over those topics.
//! * **configuration** — `/api/config`, including the broker settings. Editing
//!   your own transport over that transport is a bad way to lose a controller.
//! * **the serial terminal** — `/ws/serial/{n}`, raw bytes. A console is a byte
//!   stream with backpressure and scrollback, none of which pub/sub does well.
use crate::ops::{self, Cmd, Fault};
use crate::Config;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

type Ctx = State<Arc<Config>>;

pub fn router(cfg: Arc<Config>) -> Router {
    Router::new()
        // IO control: MQTT over a WebSocket, for the browser.
        .route("/mqtt", get(ws_mqtt))
        // A terminal wants bytes, not packets.
        .route("/ws/serial/{index}", get(ws_serial))
        // What the board is, and how it is configured.
        .route("/api/health", get(health))
        .route("/api/io", get(|s: Ctx| run(s, Cmd::Capabilities)))
        .route("/api/io/mcu", get(|s: Ctx| run(s, Cmd::McuInfo)))
        // Recovery, over REST as well as MQTT: a wedged MCU is exactly when
        // you cannot rely on the IO transport to carry the fix.
        .route("/api/io/mcu/reset", axum::routing::post(|s: Ctx| run(s, Cmd::McuReset)))
        .route("/api/config", get(get_config).post(put_config))
        .layer(axum::middleware::from_fn(cors))
        .layer(axum::middleware::from_fn(auth))
        .with_state(cfg)
}

/// Permissive CORS, deliberately.
///
/// Not for the config GUI — that reaches iod through webd's `/iod` proxy and is
/// same-origin. This is for everything else: a dashboard on another host, a
/// scratch page, an automation tool's browser client. Those are cross-origin by
/// nature, and refusing them would buy no security on a LAN appliance where
/// anything that can reach :80 can reach :7070 directly anyway. Authentication
/// is what protects this (`IOD_TOKEN`); the origin header never was.
async fn cors(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    };
    let preflight = req.method() == axum::http::Method::OPTIONS;
    let mut res = if preflight {
        axum::response::Response::new(axum::body::Body::empty())
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
    h.insert(ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS".parse().unwrap());
    h.insert(ACCESS_CONTROL_ALLOW_HEADERS, "content-type".parse().unwrap());
    res
}

/// Optional shared-secret auth, off unless `IOD_TOKEN` is set.
///
/// Off by default because the first thing this has to do is work on a bench
/// with a serial cable and no configuration. On by a single environment line
/// for anyone exposing a controller to a wider network, which is the point at
/// which "anything that can reach the LAN can close a relay" stops being
/// acceptable.
///
/// The query-parameter form exists because the browser WebSocket API cannot set
/// request headers — there is no way for a page to send a bearer token on a
/// socket handshake. It is not the weaker option by choice.
async fn auth(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let Ok(want) = std::env::var("IOD_TOKEN") else { return next.run(req).await };
    if want.is_empty() {
        return next.run(req).await;
    }
    // Liveness stays open: a monitor should be able to see that the daemon is
    // up without holding a credential, and it discloses nothing.
    if req.uri().path() == "/api/health" || req.method() == axum::http::Method::OPTIONS {
        return next.run(req).await;
    }
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let query = req.uri().query().unwrap_or("").split('&').find_map(|kv| {
        kv.strip_prefix("token=").map(|v| v.replace("%20", " "))
    });
    let got = header.or(query).unwrap_or_default();
    if constant_eq(got.as_bytes(), want.as_bytes()) {
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad or missing token", "code": "unauthorized" })))
        .into_response()
}

/// Compare without an early return, so the time taken does not reveal how much
/// of a guessed token was right.
pub fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn health() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": "iod" }))
}

// ── REST adapters ──────────────────────────────────────────────────────────
//
// Each is three lines because the logic is in ops. A rule enforced there — a
// range check, a capability check — is enforced identically here, on the
// control socket and over MQTT, because there is only one copy of it.

fn fault_response(f: Fault) -> axum::response::Response {
    let status = StatusCode::from_u16(f.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({ "error": f.to_string(), "code": f.code() }))).into_response()
}

async fn run(State(c): Ctx, cmd: Cmd) -> axum::response::Response {
    match ops::dispatch(&c, cmd).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fault_response(e),
    }
}

// ── IO control: MQTT over WebSocket ────────────────────────────────────────

/// The browser's transport to iod's MQTT endpoint.
///
/// mqtt.js on the other end; the same packets a plain-TCP client sends, just
/// carried in binary WebSocket frames. Sub-protocol negotiation is answered
/// because browsers offer `mqtt` and some clients refuse a server that does not
/// echo it back.
async fn ws_mqtt(State(c): Ctx, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.protocols(["mqtt", "mqttv3.1"]).on_upgrade(move |sock| mqtt_ws_loop(c, sock))
}

async fn mqtt_ws_loop(c: Arc<Config>, sock: WebSocket) {
    use futures_util::{SinkExt, StreamExt};
    let m = c.settings.lock().map(|s| s.mqtt.clone()).unwrap_or_default();
    if !m.serve {
        return;
    }
    let (mut ws_tx, mut ws_rx) = sock.split();
    let (in_tx, in_rx) = tokio::sync::mpsc::channel::<rumqttc::Packet>(64);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<rumqttc::Packet>(256);

    // Packets out to the browser.
    tokio::spawn(async move {
        while let Some(p) = out_rx.recv().await {
            let Some(bytes) = crate::mqtt::server::encode(&p) else { continue };
            if ws_tx.send(Message::Binary(bytes.into())).await.is_err() {
                return;
            }
        }
    });
    // Packets in. A WebSocket frame boundary has nothing to do with a packet
    // boundary, so the bytes are reassembled before being decoded.
    tokio::spawn(async move {
        let mut framer = crate::mqtt::server::Framer::new();
        while let Some(Ok(msg)) = ws_rx.next().await {
            let bytes = match msg {
                Message::Binary(b) => b.to_vec(),
                Message::Text(t) => t.as_bytes().to_vec(),
                Message::Close(_) => return,
                _ => continue,
            };
            framer.feed(&bytes);
            loop {
                match framer.next() {
                    Ok(Some(p)) => {
                        if in_tx.send(p).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("iod/mqttd: bad packet from a browser: {e}");
                        return;
                    }
                }
            }
        }
    });

    crate::mqtt::server::session(c, m, in_rx, out_tx, "browser".into()).await;
}

// ── configuration ──────────────────────────────────────────────────────────

/// Settings, minus anything secret.
///
/// The password is never sent back. A GUI does not need it to render a settings
/// form — it needs to know whether one is SET — and echoing a broker credential
/// to every page load is a needless way to leak it.
async fn get_config(State(c): Ctx) -> impl IntoResponse {
    let s = c.settings.lock().unwrap();
    let m = &s.mqtt;
    Json(json!({
        "mqtt": {
            "serve": m.serve,
            "listen_port": m.listen_port,
            "bridge": m.bridge,
            "url": m.url,
            "username": m.username,
            "password_set": !m.password.is_empty(),
            "ca_path": m.ca_path,
            "client_cert_path": m.client_cert_path,
            "client_key_path": m.client_key_path,
            "prefix": m.prefix,
            "client_id": m.client_id,
            "discovery": m.discovery,
        },
        // Set in the environment, so the UI shows them as read-only instead of
        // accepting an edit that would be silently overridden on restart.
        "pinned": c.pinned,
        "topics": {
            "base": crate::mqtt::topics::base(&m.prefix, &m.client_id),
        },
    }))
}

#[derive(Deserialize)]
struct ConfigReq {
    mqtt: serde_json::Value,
}

async fn put_config(State(c): Ctx, Json(req): Json<ConfigReq>) -> axum::response::Response {
    let mut next = {
        let s = c.settings.lock().unwrap();
        s.mqtt.clone()
    };
    // Merge field by field rather than deserialising the whole struct, so a
    // client that omits `password` keeps the stored one instead of blanking it.
    let o = match req.mqtt.as_object() {
        Some(o) => o,
        None => return fault_response(Fault::Bad("mqtt must be an object".into())),
    };
    let s_of = |k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_string);
    if let Some(v) = o.get("serve").and_then(|v| v.as_bool()) { next.serve = v }
    if let Some(v) = o.get("bridge").and_then(|v| v.as_bool()) { next.bridge = v }
    if let Some(v) = o.get("listen_port").and_then(|v| v.as_u64()) {
        if v > u16::MAX as u64 {
            return fault_response(Fault::Bad("listen_port out of range".into()));
        }
        next.listen_port = v as u16;
    }
    if let Some(v) = s_of("url") { next.url = v }
    if let Some(v) = s_of("username") { next.username = v }
    if let Some(v) = s_of("password") { next.password = v }
    if let Some(v) = s_of("ca_path") { next.ca_path = v }
    if let Some(v) = s_of("client_cert_path") { next.client_cert_path = v }
    if let Some(v) = s_of("client_key_path") { next.client_key_path = v }
    if let Some(v) = s_of("discovery") { next.discovery = v }
    if let Some(v) = s_of("prefix") {
        if v.trim().is_empty() || v.contains(['+', '#']) {
            return fault_response(Fault::Bad("prefix must be non-empty and free of + and #".into()));
        }
        next.prefix = v;
    }
    if let Some(v) = s_of("client_id") {
        if v.trim().is_empty() || v.contains(['+', '#', '/']) {
            return fault_response(Fault::Bad("client_id must be non-empty and free of + # and /".into()));
        }
        next.client_id = v;
    }
    if next.bridge && next.url.trim().is_empty() {
        return fault_response(Fault::Bad("bridging needs a broker URL".into()));
    }

    {
        let mut s = c.settings.lock().unwrap();
        s.mqtt = next.clone();
        if let Err(e) = s.save() {
            // Applied in memory but not persisted: say so rather than
            // reporting success and losing it at the next reboot.
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("settings applied but not saved: {e}"), "code": "save_failed" })),
            )
                .into_response();
        }
    }
    // Restarts whichever roles are affected.
    let _ = c.settings_tx.send(next);
    Json(json!({ "ok": true })).into_response()
}

// ── the terminal socket ────────────────────────────────────────────────────

async fn ws_serial(State(c): Ctx, Path(index): Path<usize>, ws: WebSocketUpgrade) -> impl IntoResponse {
    let Some(port) = c.board.io.serials.get(index).cloned() else {
        return fault_response(Fault::NoSuch(format!("no serial port {index}")));
    };
    // MCU-routed ports have no device node; their bytes travel over the IO
    // protocol's UART opcodes, which is a different path entirely. Say so
    // rather than failing to open a device that was never going to exist.
    let Some(dev) = port.dev.clone() else {
        return fault_response(Fault::Todo(format!(
            "port {index} is {}-routed; that bridge is not implemented yet", port.transport)));
    };
    let baud = c
        .bus
        .state
        .get(&format!("serial/{index}/baud"))
        .and_then(|v| v.as_u64())
        .map(|b| b as u32)
        .unwrap_or(port.baud);
    let sess = match c.serial.session(index, &dev, baud, &c.bus) {
        Ok(s) => s,
        Err(e) => return fault_response(Fault::Io(format!("cannot open {dev}: {e}"))),
    };
    let c2 = c.clone();
    ws.on_upgrade(move |sock| serial_loop(sock, sess, c2))
}

/// Attach a terminal to the shared session.
///
/// Every viewer of a port sees the same stream and any of them can type into
/// it, because there is one session per port rather than one per connection.
/// Two installers on the same console see each other's keystrokes, which is
/// what makes it a shared console instead of two people fighting over a cable.
async fn serial_loop(mut sock: WebSocket, sess: Arc<crate::serial::Session>, c: Arc<Config>) {
    let mut rx = sess.subscribe();
    let n = sess.join(&c.bus);

    // Replay recent history so joining mid-session does not mean staring at a
    // blank screen until the far end next says something.
    let hist = sess.history();
    if !hist.is_empty() && sock.send(Message::Binary(hist.into())).await.is_err() {
        sess.leave(&c.bus);
        return;
    }
    let hello = format!(
        "\r\n[iod] {} @ {} baud — {} viewer{}\r\n",
        sess.dev,
        sess.baud(),
        n,
        if n == 1 { "" } else { "s" }
    );
    let _ = sock.send(Message::Binary(hello.into_bytes().into())).await;

    loop {
        tokio::select! {
            // UART -> browser. Binary, because a terminal stream is bytes: text
            // frames would force UTF-8 validation on data that is not text.
            r = rx.recv() => match r {
                Ok(b) => {
                    if sock.send(Message::Binary(b.into())).await.is_err() { break }
                }
                // Dropped bytes are visible as a gap rather than a disconnect;
                // for a console, a gap is the lesser evil.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            // browser -> UART. xterm sends keystrokes as text; anything binary
            // is passed through untouched.
            m = sock.recv() => match m {
                Some(Ok(Message::Binary(b))) => sess.write(b.to_vec()),
                Some(Ok(Message::Text(t)))   => sess.write(t.as_bytes().to_vec()),
                Some(Ok(_)) => {}
                _ => break,
            },
        }
    }
    sess.leave(&c.bus);
}
