//! The IO API.
//!
//! Shaped around one rule taken from the house UI: **a thing with nothing
//! behind it does not appear.** A board with no relays does not get an empty
//! relay list, it gets no `relays` key at all, so a client can render purely
//! from the capability document without special-casing each model.
use crate::events::Event;
use crate::{board::Backend, Config};
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
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

type Ctx = State<Arc<Config>>;

pub fn router(cfg: Arc<Config>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/io", get(capabilities))
        .route("/api/io/mcu", get(mcu_info))
        .route("/api/io/contacts", get(contacts))
        .route("/api/io/relays", get(relays))
        .route("/api/io/relays/toggle", post(relay_toggle))
        .route("/api/io/ir/{port}/send", post(ir_send))
        // Live state. Everything above is a question; these two are the answers
        // that arrive without being asked.
        .route("/ws/events", get(ws_events))
        .route("/ws/serial/{index}", get(ws_serial))
        .with_state(cfg)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": "iod" }))
}

/// Everything a client needs to draw the UI, and nothing it does not.
async fn capabilities(State(c): Ctx) -> impl IntoResponse {
    let io = &c.board.io;
    let mut v = json!({
        "board":    c.board.model,
        "hostname": c.board.hostname,
        "backend":  io.backend,
        "mcu_linked": c.link.is_some(),
    });
    let m = v.as_object_mut().unwrap();

    // Each section appears only if the board has one.
    if io.ir_total() > 0 {
        m.insert("ir".into(), json!({
            "out": io.ir_out,
            "blaster": io.ir_blaster,
            "total": io.ir_total(),
            // Combo ports share connectors with the user UARTs: a port is IR or
            // serial, never both. A client must not offer the same index twice.
            "combo": io.ir_combo,
            "receiver": io.ir_in,
        }));
    }
    if io.relays > 0 {
        m.insert("relays".into(), json!({ "count": io.relays }));
    }
    if io.contacts > 0 {
        m.insert("contacts".into(), json!({ "count": io.contacts }));
    }
    if !io.serials.is_empty() {
        m.insert("serials".into(), json!(io.serials));
    }
    Json(v)
}

fn no_mcu() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "no IO microcontroller on this board, or its port could not be opened" })),
    )
}

async fn mcu_info(State(c): Ctx) -> impl IntoResponse {
    let Some(l) = &c.link else { return no_mcu().into_response() };
    let mut l = l.lock().await;
    let ident = l.identify();
    let baud = l.measured_baud().ok();
    match ident {
        Ok((product, version)) => Json(json!({
            "part": l.part, "baud": l.baud, "product": product, "version": version,
            // What the MCU says it measured, BE32. Handy as a link check: an
            // HC-800 reports ~115207 against a nominal 115200.
            "measured_baud": baud,
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn contacts(State(c): Ctx) -> impl IntoResponse {
    if c.board.io.contacts == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this board has no contacts" }))).into_response();
    }
    match &c.board.io.backend {
        Backend::Mcu => {
            let Some(l) = &c.link else { return no_mcu().into_response() };
            match l.lock().await.contacts() {
                Ok(bits) => {
                    let n = c.board.io.contacts;
                    // Bit N = contact N; 1 = CLOSED.
                    let states: Vec<bool> = (0..n).map(|i| bits >> i & 1 == 1).collect();
                    Json(json!({ "mask": bits, "closed": states })).into_response()
                }
                Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
            }
        }
        // The IO Extender reads its contacts as plain GPIO lines. Not wired up
        // yet; saying so beats reporting all-open, which is a valid-looking lie.
        Backend::Gpio => (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "gpio contact backend not implemented", "lines": c.board.io.contact_gpios })),
        )
            .into_response(),
        Backend::None => (StatusCode::NOT_FOUND, Json(json!({ "error": "no IO on this board" }))).into_response(),
    }
}

async fn relays(State(c): Ctx) -> impl IntoResponse {
    if c.board.io.relays == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this board has no relays" }))).into_response();
    }
    let Some(l) = &c.link else { return no_mcu().into_response() };
    match l.lock().await.relays_raw() {
        // Raw, because the encoding is not decoded: a four-relay HC-800 answers
        // `ff 00`, which is not a per-relay bitmap. See link.rs.
        Ok(raw) => Json(json!({ "count": c.board.io.relays, "raw": raw, "decoded": false })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct ToggleReq {
    mask: u8,
}

async fn relay_toggle(State(c): Ctx, Json(req): Json<ToggleReq>) -> impl IntoResponse {
    let n = c.board.io.relays;
    if n == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this board has no relays" }))).into_response();
    }
    // Refuse bits that do not correspond to a relay rather than passing them to
    // the firmware and hoping. A relay may be switching a real load.
    let valid: u8 = if n >= 8 { 0xff } else { (1u8 << n) - 1 };
    if req.mask & !valid != 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("mask 0x{:02x} selects relays this board does not have (valid 0x{:02x})", req.mask, valid) })),
        )
            .into_response();
    }
    let Some(l) = &c.link else { return no_mcu().into_response() };
    match l.lock().await.relay_toggle(req.mask) {
        Ok(raw) => Json(json!({ "raw": raw })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct IrSend {
    /// Pronto hex, space separated. Only code type 0000 is accepted by the
    /// firmware; anything else is rejected there rather than here.
    pronto: String,
    #[serde(default = "one")]
    repeat: u8,
}
fn one() -> u8 {
    1
}

#[derive(Serialize)]
struct Accepted {
    port: u8,
    words: usize,
}

async fn ir_send(State(c): Ctx, Path(port): Path<u8>, Json(req): Json<IrSend>) -> impl IntoResponse {
    let total = c.board.io.ir_total();
    if total == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this board has no IR outputs" }))).into_response();
    }
    if port >= total {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("port {port} out of range (0..{})", total - 1) })),
        )
            .into_response();
    }
    let words: Result<Vec<u16>, _> =
        req.pronto.split_whitespace().map(|w| u16::from_str_radix(w, 16)).collect();
    let Ok(words) = words else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "pronto must be space-separated hex words" }))).into_response();
    };
    if words.first() != Some(&0) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "only Pronto code type 0000 (raw, learned) is supported" })),
        )
            .into_response();
    }
    let Some(l) = &c.link else { return no_mcu().into_response() };

    // Payload layout is the vendor's IROUT_SEND: port, repeat, then the Pronto
    // words big-endian. Burst durations are CARRIER PERIODS, not microseconds.
    let mut payload = Vec::with_capacity(2 + words.len() * 2);
    payload.push(port);
    payload.push(req.repeat);
    for w in &words {
        payload.extend_from_slice(&w.to_be_bytes());
    }
    let mut l = l.lock().await;
    match l.request(crate::mcu::OP_IROUT_SEND, &payload, Duration::from_secs(3)) {
        Ok(f) => Json(json!({ "accepted": Accepted { port, words: words.len() }, "status": f.payload })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

// ── live streams ───────────────────────────────────────────────────────────

async fn ws_events(State(c): Ctx, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |sock| events_loop(c, sock))
}

async fn events_loop(c: Arc<Config>, mut sock: WebSocket) {
    let mut rx = c.bus.subscribe();

    // Open with a snapshot so a client that connects between transitions still
    // renders the truth. Without this a freshly-loaded page shows every contact
    // as open until one happens to change, which can be hours.
    if c.board.io.contacts > 0 {
        if let Some(l) = &c.link {
            if let Ok(mask) = l.lock().await.contacts() {
                let closed = (0..c.board.io.contacts).map(|i| mask >> i & 1 == 1).collect();
                let snap = Event::ContactSnapshot { mask, closed };
                if let Ok(t) = serde_json::to_string(&snap) {
                    let _ = sock.send(Message::Text(t.into())).await;
                }
            }
        }
    }

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(e) => {
                    let Ok(t) = serde_json::to_string(&e) else { continue };
                    if sock.send(Message::Text(t.into())).await.is_err() {
                        return; // client went away
                    }
                }
                // Lagged: this client did not keep up and the bus dropped the
                // oldest for it. Keep going rather than dropping the socket —
                // the next event still gets through, and for a live view a gap
                // is better than a disconnect.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            // Drain client frames so a close/ping is noticed promptly.
            msg = sock.recv() => match msg {
                Some(Ok(_)) => {}
                _ => return,
            },
        }
    }
}

async fn ws_serial(State(c): Ctx, Path(index): Path<usize>, ws: WebSocketUpgrade) -> impl IntoResponse {
    let Some(port) = c.board.io.serials.get(index).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no such serial port" }))).into_response();
    };
    // MCU-routed ports have no device node; their bytes travel over the IO
    // protocol's UART opcodes, which is a different path entirely. Say so
    // rather than failing to open a device that was never going to exist.
    let Some(dev) = port.dev.clone() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": "this port is MCU-routed; the protocol bridge is not implemented yet",
                         "transport": port.transport })),
        )
            .into_response();
    };
    ws.on_upgrade(move |sock| serial_loop(sock, dev, port.baud))
}

async fn serial_loop(mut sock: WebSocket, dev: String, baud: u32) {
    let port = match crate::serial::Port::open(&dev, baud) {
        Ok(p) => p,
        Err(e) => {
            let _ = sock.send(Message::Text(format!("iod: cannot open {dev}: {e}\r\n").into())).await;
            return;
        }
    };
    let mut buf = [0u8; 1024];
    loop {
        tokio::select! {
            // UART -> browser. Binary, because a terminal stream is bytes: text
            // frames would force UTF-8 validation on data that is not text.
            r = port.read(&mut buf) => match r {
                Ok(0) => {}
                Ok(n) => {
                    if sock.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            // browser -> UART. xterm sends keystrokes as text; anything binary
            // is passed through untouched.
            m = sock.recv() => match m {
                Some(Ok(Message::Binary(b))) => { if port.write_all(&b).await.is_err() { return } }
                Some(Ok(Message::Text(t)))   => { if port.write_all(t.as_bytes()).await.is_err() { return } }
                Some(Ok(_)) => {}
                _ => return,
            },
        }
    }
}
