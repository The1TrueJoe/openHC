//! The wire surfaces. All three of them are adapters over [`crate::ops`].
//!
//! * `/ws/control` — the real one. Commands, state and events on a single
//!   socket. This is what webd's panels use and what an external control system
//!   should use: connect once, get a state snapshot, subscribe to the events you
//!   care about, issue commands with correlation ids.
//! * REST — the same commands, one per request, for scripts and curl. Stateless
//!   and convenient; it cannot deliver events, so anything reactive wants the
//!   socket instead.
//! * `/ws/serial/{n}` — raw bytes for a terminal. Not a separate implementation:
//!   it attaches to the same shared session the control socket reports on.
use crate::events::{matches, Msg};
use crate::ops::{self, Cmd, Fault};
use crate::Config;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Ctx = State<Arc<Config>>;

pub fn router(cfg: Arc<Config>) -> Router {
    Router::new()
        // The primary surface.
        .route("/ws/control", get(ws_control))
        // A terminal wants bytes, not JSON.
        .route("/ws/serial/{index}", get(ws_serial))
        // REST, for everything that is a one-shot question.
        .route("/api/health", get(health))
        .route("/api/io", get(|s: Ctx| run(s, Cmd::Capabilities)))
        .route("/api/state", get(|s: Ctx| run(s, Cmd::StateGet)))
        .route("/api/io/mcu", get(|s: Ctx| run(s, Cmd::McuInfo)))
        .route("/api/io/contacts", get(|s: Ctx| run(s, Cmd::ContactGet)))
        .route("/api/io/relays", get(|s: Ctx| run(s, Cmd::RelayGet)))
        .route("/api/io/relays/set", post(relay_set))
        .route("/api/io/relays/toggle", post(relay_toggle))
        .route("/api/io/ir/{port}/send", post(ir_send))
        .route("/api/io/serial", get(|s: Ctx| run(s, Cmd::SerialList)))
        .route("/api/io/serial/{index}/baud", post(serial_baud))
        .route("/api/io/serial/{index}/write", post(serial_write))
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
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
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

#[derive(Deserialize)]
struct RelaySetReq {
    /// Relay index, 0-based. Not a mask: the protocol addresses one relay at a
    /// time and answers with that relay's state, so an API taking a mask could
    /// not report back what it had done.
    index: u8,
    on: bool,
}
#[derive(Deserialize)]
struct RelayIdx {
    index: u8,
}
#[derive(Deserialize)]
struct IrSendReq {
    pronto: String,
    #[serde(default = "one")]
    repeat: u8,
}
fn one() -> u8 {
    1
}
#[derive(Deserialize)]
struct BaudReq {
    baud: u32,
}
#[derive(Deserialize)]
struct WriteReq {
    data: String,
    #[serde(default)]
    hex: bool,
    #[serde(default)]
    b64: bool,
}

async fn relay_set(s: Ctx, Json(r): Json<RelaySetReq>) -> axum::response::Response {
    run(s, Cmd::RelaySet { index: r.index, on: r.on }).await
}
async fn relay_toggle(s: Ctx, Json(r): Json<RelayIdx>) -> axum::response::Response {
    run(s, Cmd::RelayToggle { index: r.index }).await
}
async fn ir_send(s: Ctx, Path(port): Path<u8>, Json(r): Json<IrSendReq>) -> axum::response::Response {
    run(s, Cmd::IrSend { port, pronto: r.pronto, repeat: r.repeat }).await
}
async fn serial_baud(s: Ctx, Path(index): Path<usize>, Json(r): Json<BaudReq>) -> axum::response::Response {
    run(s, Cmd::SerialBaud { index, baud: r.baud }).await
}
async fn serial_write(s: Ctx, Path(index): Path<usize>, Json(r): Json<WriteReq>) -> axum::response::Response {
    run(s, Cmd::SerialWrite { index, data: r.data, hex: r.hex, b64: r.b64 }).await
}

// ── the control socket ─────────────────────────────────────────────────────

/// A frame from a client. Either a socket-level instruction or a command.
#[derive(Deserialize)]
struct Incoming {
    /// Echoed back on the reply so a client can have several in flight.
    #[serde(default)]
    id: Option<Value>,
    #[serde(flatten)]
    body: Value,
}

async fn ws_control(State(c): Ctx, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |sock| control_loop(c, sock))
}

async fn control_loop(c: Arc<Config>, mut sock: WebSocket) {
    let mut rx = c.bus.subscribe();
    // Event subscriptions, empty by default.
    //
    // State is different and always flows: it is small, everyone needs it, and
    // a client holding a stale mirror is actively wrong. Events are opt-in
    // because they are not — the byte stream of a chatty projector must not be
    // pushed at a client that only wanted to know about contacts.
    let mut filters: Vec<String> = Vec::new();

    // Open with the state document so the client starts from truth rather than
    // guessing until something changes — which for a quiet contact can be hours.
    if let Ok(t) = serde_json::to_string(&c.bus.snapshot()) {
        if sock.send(Message::Text(t.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(env) => {
                    let deliver = match &env.msg {
                        // State and snapshots always go.
                        Msg::State { .. } | Msg::Snapshot { .. } => true,
                        Msg::Event { topic, .. } => filters.iter().any(|f| matches(f, topic)),
                    };
                    if !deliver { continue }
                    let Ok(t) = serde_json::to_string(&env) else { continue };
                    if sock.send(Message::Text(t.into())).await.is_err() {
                        return; // client went away
                    }
                }
                // This client fell behind and the bus dropped the oldest for
                // it. Say so explicitly and resend the state document: a
                // control system that silently missed a relay change would act
                // on a stale mirror, which is worse than a visible gap.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let _ = sock.send(Message::Text(
                        json!({ "type": "lagged", "missed": n }).to_string().into())).await;
                    if let Ok(t) = serde_json::to_string(&c.bus.snapshot()) {
                        if sock.send(Message::Text(t.into())).await.is_err() { return }
                    }
                }
                Err(_) => return,
            },
            msg = sock.recv() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Binary(b))) => String::from_utf8_lossy(&b).to_string(),
                    Some(Ok(_)) => continue,
                    _ => return,
                };
                let reply = handle_frame(&c, &text, &mut filters).await;
                if let Some(r) = reply {
                    if sock.send(Message::Text(r.to_string().into())).await.is_err() { return }
                }
            }
        }
    }
}

/// One client frame. Returns the reply to send, if any.
async fn handle_frame(c: &Arc<Config>, text: &str, filters: &mut Vec<String>) -> Option<Value> {
    let inc: Incoming = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return Some(json!({ "ok": false, "error": { "code": "bad_json", "message": e.to_string() } })),
    };
    let id = inc.id.clone();
    let op = inc.body.get("op").and_then(|v| v.as_str()).unwrap_or("");

    // Socket-level operations, handled here because they are about THIS
    // connection rather than about the hardware.
    let socket_level = match op {
        "subscribe" => {
            let topics = topics_of(&inc.body);
            for t in topics {
                if !filters.contains(&t) {
                    filters.push(t);
                }
            }
            Some(json!({ "subscribed": filters.clone() }))
        }
        "unsubscribe" => {
            let topics = topics_of(&inc.body);
            filters.retain(|f| !topics.contains(f));
            Some(json!({ "subscribed": filters.clone() }))
        }
        "ping" => Some(json!({ "pong": true })),
        _ => None,
    };
    if let Some(result) = socket_level {
        return Some(json!({ "id": id, "ok": true, "result": result }));
    }

    let cmd: Cmd = match serde_json::from_value(inc.body) {
        Ok(c) => c,
        Err(e) => {
            return Some(json!({ "id": id, "ok": false,
                "error": { "code": "bad_request", "message": e.to_string() } }))
        }
    };
    match ops::dispatch(c, cmd).await {
        Ok(v) => Some(json!({ "id": id, "ok": true, "result": v })),
        Err(f) => Some(json!({ "id": id, "ok": false,
            "error": { "code": f.code(), "status": f.status(), "message": f.to_string() } })),
    }
}

/// `topics` as a list, or `topic` as a single string. Both spellings appear in
/// the wild and rejecting one is a pointless way to fail.
fn topics_of(body: &Value) -> Vec<String> {
    if let Some(a) = body.get("topics").and_then(|v| v.as_array()) {
        return a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    body.get("topic").and_then(|v| v.as_str()).map(|s| vec![s.to_string()]).unwrap_or_default()
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
