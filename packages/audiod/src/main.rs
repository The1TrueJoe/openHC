//! ohc-audiod — the openHC audio daemon.
//!
//! Owns audio end to end, with its own MQTT presence (no IPC through iod):
//!
//! * **MQTT** (live state and control, what a Control4 / Home Assistant
//!   integration watches or drives): `<base>/state/audio/map` — the endpoint
//!   map with every instance's running state; `audio/output`, `audio/volume`,
//!   `audio/receiver/<id>/running` on single-output boards; commands
//!   `<base>/cmd/audio/output` and `<base>/cmd/audio/volume`.
//! * **REST** (configuration, which changes rarely): `GET /api/audio` and
//!   `PUT /api/audio/endpoints` — which Spotify Connect and AirPlay endpoints
//!   exist and which output each plays on, and live input routes. webd proxies
//!   it at `/audio/`.
//!
//! On a board with named outputs (board.env OHC_AUDIO_OUTPUTS — the HC-800) it
//! runs every endpoint itself (see runner.rs); elsewhere it reports the stock
//! single receivers and turns their output/volume knobs (single.rs).
mod board;
mod config;
mod runner;
mod single;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use board::Board;
use config::Endpoints;
use ohcmqtt::{Base, Retained};
use rumqttc::{AsyncClient, Event, Incoming, QoS};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(OpenApi)]
#[openapi(
    info(title = "ohc-audiod", description = "Audio: Spotify Connect + AirPlay endpoints and input routes mapped onto outputs. Configuration here; live state and control over MQTT (<base>/state/audio/*, <base>/cmd/audio/*)."),
    tags((name = "Audio", description = "Outputs, inputs and the endpoint map."))
)]
struct ApiDoc;

struct App {
    board: Board,
    endpoints: Mutex<Endpoints>,
    instances: Mutex<Vec<runner::Instance>>,
    up: runner::Up,
    /// Poked whenever published state may have changed.
    changed: Notify,
}

impl App {
    fn restart(&self) {
        let e = self.endpoints.lock().unwrap().clone();
        let mut inst = self.instances.lock().unwrap();
        inst.clear(); // drops = stops
        if self.board.has_map() {
            *inst = runner::start(&self.board, &e, &self.up);
        }
    }

    /// The endpoint map document: configuration plus what is running.
    fn map(&self) -> Option<Value> {
        if !self.board.has_map() {
            return None;
        }
        let up = self.up.lock().unwrap();
        let instances: Vec<Value> = self
            .instances
            .lock()
            .unwrap()
            .iter()
            .map(|i| json!({
                "tag": i.tag, "kind": i.kind, "name": i.name, "input": i.input, "output": i.output,
                "running": up.get(&i.tag).copied().unwrap_or(false),
            }))
            .collect();
        let e = self.endpoints.lock().unwrap();
        Some(json!({
            "outputs": self.board.outputs,
            "inputs": self.board.inputs,
            "rate": self.board.rate,
            "spotify": e.spotify,
            "airplay": e.airplay,
            "routes": e.routes,
            "instances": instances,
        }))
    }
}

type Ctx = State<Arc<App>>;

#[utoipa::path(get, path = "/api/audio", tag = "Audio",
    summary = "Outputs, inputs, the endpoint map and the receivers",
    description = "`map` is present on boards with named outputs: outputs/inputs (hardware, from board.env), the configured spotify/airplay/routes lists, and `instances` with each one's running state. `outputs`/`receivers`/`selected`/`volume` describe the stock single receivers (boards without named outputs). For drawing a page once — live changes arrive over MQTT.",
    responses((status = 200, description = "audio configuration and snapshot")))]
async fn get_audio(State(app): Ctx) -> Json<Value> {
    Json(json!({
        "map": app.map(),
        "outputs": single::outputs(),
        "receivers": single::receivers().into_iter().map(|r| {
            let mut v = json!(r);
            if let Some(np) = single::now_playing(r.id) {
                v["now_playing"] = np;
            }
            v
        }).collect::<Vec<_>>(),
        "selected": single::selected(),
        "volume": single::volume_get(None).await,
    }))
}

#[utoipa::path(put, path = "/api/audio/endpoints", tag = "Audio",
    summary = "Replace the endpoint map",
    description = "Validated against the board's outputs and inputs, saved persistently, and every endpoint restarted. Returns the new map.",
    request_body = Endpoints,
    responses((status = 200, description = "the new map"), (status = 400, description = "invalid: unknown output/input or a bad name"), (status = 404, description = "this board has no named outputs")))]
async fn put_endpoints(State(app): Ctx, Json(next): Json<Endpoints>) -> axum::response::Response {
    if !app.board.has_map() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this board has no named audio outputs" }))).into_response();
    }
    if let Err(e) = next.validate(&app.board) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    }
    if let Err(e) = next.save() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("save: {e}") }))).into_response();
    }
    *app.endpoints.lock().unwrap() = next;
    app.restart();
    app.changed.notify_one();
    Json(app.map()).into_response()
}

#[utoipa::path(get, path = "/api/health", tag = "Audio", summary = "ohc-audiod liveness",
    responses((status = 200, description = "ok")))]
async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "ohc-audiod" }))
}

/// Publish everything that changes. Retained drops unchanged values.
async fn publish(app: &App, client: &AsyncClient, base: &Base, r: &mut Retained) {
    if let Some(m) = app.map() {
        r.set(client, base, "audio/map", &m).await;
    }
    if let Some(dev) = single::selected() {
        r.set(client, base, "audio/output", &json!(dev)).await;
    }
    if let Some(v) = single::volume_get(None).await {
        r.set(client, base, "audio/volume", &json!(v)).await;
    }
    for rc in single::receivers() {
        r.set(client, base, &format!("audio/receiver/{}/running", rc.id), &json!(rc.running)).await;
    }
}

/// Handle `<base>/cmd/audio/<what>`.
async fn command(app: &App, what: &str, body: &str) {
    let body = body.trim();
    match what {
        "output" if !app.board.has_map() => match single::select(body) {
            Ok(_) => {
                // The stock receivers read the selection at start.
                for s in ["/etc/init.d/S95librespot", "/etc/init.d/S99shairport-sync"] {
                    let _ = tokio::process::Command::new(s).arg("restart").status().await;
                }
            }
            Err(e) => eprintln!("audiod: cmd/audio/output: {e}"),
        },
        "volume" => match body.parse::<u8>() {
            Ok(p) => {
                if let Err(e) = single::volume_set(None, p).await {
                    eprintln!("audiod: cmd/audio/volume: {e}");
                }
            }
            Err(_) => eprintln!("audiod: cmd/audio/volume: not a percent: {body:?}"),
        },
        other => eprintln!("audiod: unknown command audio/{other}"),
    }
    app.changed.notify_one();
}

async fn mqtt(app: Arc<App>) {
    let base = Base::resolve();
    let (client, mut events) = AsyncClient::new(ohcmqtt::options("audiod", &base), 32);
    let reconnected = Arc::new(Notify::new());
    eprintln!("audiod: mqtt {}:{} under {}", ohcmqtt::broker().0, ohcmqtt::broker().1, base.root());

    // Publisher: on a timer, on a state change, and everything on reconnect.
    {
        let (app, client, base, reconnected) = (app.clone(), client.clone(), base.clone(), reconnected.clone());
        tokio::spawn(async move {
            let mut r = Retained::new();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                    _ = app.changed.notified() => {}
                    _ = reconnected.notified() => { r.republish(&client, &base).await; }
                }
                publish(&app, &client, &base, &mut r).await;
            }
        });
    }

    let prefix = base.cmd("audio/");
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                ohcmqtt::online(&client, "audiod", &base).await;
                let _ = client.subscribe(base.cmd("audio/#"), QoS::AtLeastOnce).await;
                reconnected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(p))) => {
                if let Some(what) = p.topic.strip_prefix(&prefix) {
                    command(&app, what, &String::from_utf8_lossy(&p.payload)).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("audiod: mqtt: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

fn main() {
    let bind = std::env::var("AUDIOD_BIND").unwrap_or_else(|_| "0.0.0.0:7072".into());
    let board = Board::from_env();
    let endpoints = Endpoints::load();
    if board.has_map() {
        if let Err(e) = endpoints.validate(&board) {
            eprintln!("audiod: saved map invalid ({e}); endpoints that do not match the board are skipped");
        }
    }
    let app = Arc::new(App {
        board,
        endpoints: Mutex::new(endpoints),
        instances: Mutex::new(Vec::new()),
        up: Default::default(),
        changed: Notify::new(),
    });

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    rt.block_on(async move {
        app.restart();
        eprintln!(
            "audiod: {} outputs, {} inputs, {} instances",
            app.board.outputs.len(),
            app.board.inputs.len(),
            app.instances.lock().unwrap().len()
        );
        tokio::spawn(mqtt(app.clone()));

        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(get_audio))
            .routes(routes!(put_endpoints))
            .routes(routes!(health))
            .with_state(app)
            .split_for_parts();
        let router = router.route(
            "/api/openapi.json",
            axum::routing::get(move || {
                let api = api.clone();
                async move { Json(api) }
            }),
        );
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("audiod: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        eprintln!("audiod: REST on {bind}");
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        if let Err(e) = axum::serve(listener, router).with_graceful_shutdown(shutdown).await {
            eprintln!("audiod: {e}");
        }
    });
}
