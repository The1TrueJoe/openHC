//! The IO API.
//!
//! Shaped around one rule taken from the house UI: **a thing with nothing
//! behind it does not appear.** A board with no relays does not get an empty
//! relay list, it gets no `relays` key at all, so a client can render purely
//! from the capability document without special-casing each model.
use crate::{board::Backend, Config};
use axum::{
    extract::{Path, State},
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
