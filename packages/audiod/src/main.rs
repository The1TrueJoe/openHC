//! ohc-audiod — the openHC audio daemon.
//!
//! Owns audio end to end, with its own MQTT presence (no IPC through iod):
//!
//! * **MQTT** (live state and control, what a Control4 / Home Assistant
//!   integration watches or drives): `<base>/state/audio/map` — the endpoint
//!   map with every instance's running state; `audio/output`, `audio/volume`,
//!   `audio/receiver/<id>/running` on single-output boards; commands
//!   `<base>/cmd/audio/output` and `<base>/cmd/audio/volume`. With named
//!   outputs, each output's level `audio/level/<id>` (0-100, also what its
//!   AirPlay/Spotify endpoints' volume sliders set) and `audio/announcing/<id>`;
//!   commands `cmd/audio/level/<id>` and `cmd/audio/announce/<id|all>` (see
//!   levels.rs and announce.rs); every Spotify/AirPlay endpoint's now-playing
//!   on `audio/meta/<tag>` (meta.rs).
//! * **REST** (configuration, which changes rarely): `GET /api/audio` and
//!   `PUT /api/audio/endpoints` — which Spotify Connect and AirPlay endpoints
//!   exist and which output each plays on, and live input routes. webd proxies
//!   it at `/audio/`.
//!
//! On a board with named outputs (board.env OHC_AUDIO_OUTPUTS — the HC-800) it
//! runs every endpoint itself (see runner.rs); elsewhere it reports the stock
//! single receivers and turns their output/volume knobs (single.rs).
mod announce;
mod board;
mod config;
mod levels;
mod meta;
mod runner;
mod single;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use board::Board;
use config::Endpoints;
use ohcmqtt::{Base, Retained};
use rumqttc::{AsyncClient, Event, Incoming, QoS};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::IntoFuture;
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
    /// Output id → level percent, as last set or read back from the control.
    levels: Mutex<HashMap<String, u8>>,
    /// Outputs with an announcement playing.
    announcing: Mutex<HashSet<String>>,
    /// AirPlay endpoints whose metadata reader thread is running.
    readers: Mutex<HashSet<String>>,
    /// One announcement at a time per output; the rest wait their turn.
    announce_turn: HashMap<String, tokio::sync::Mutex<()>>,
    /// Poked whenever published state may have changed.
    changed: Notify,
}

impl App {
    fn restart(self: &Arc<Self>) {
        let e = self.endpoints.lock().unwrap().clone();
        let mut inst = self.instances.lock().unwrap();
        inst.clear(); // drops = stops
        if self.board.has_map() {
            runner::write_asound(&self.board);
            levels::prepare(&self.board, &self.levels.lock().unwrap());
            *inst = runner::start(&self.board, &e, &self.up);
            // One metadata reader per AirPlay endpoint, for the life of the
            // daemon (its FIFO outlives a shairport-sync restart).
            for i in inst.iter().filter(|i| i.kind == "airplay") {
                if self.readers.lock().unwrap().insert(i.tag.clone()) {
                    let app = self.clone();
                    meta::read_shairport(i.tag.clone(), move || app.changed.notify_one());
                }
            }
        }
    }

    fn set_level(&self, id: &str, percent: u8) -> Result<(), String> {
        let p = self.board.output(id).ok_or_else(|| format!("no output '{id}'"))?;
        let percent = percent.min(100);
        levels::cset(p, &levels::vol_control(p), levels::percent_to_raw(percent));
        let mut l = self.levels.lock().unwrap();
        l.insert(id.to_string(), percent);
        levels::save(&l);
        Ok(())
    }

    /// Pick up level changes made outside this daemon — an AirPlay or Spotify
    /// volume slider moving the control. A raw value that still matches the
    /// saved percent keeps that percent (the taper is many-to-one at the
    /// bottom, so reading back would otherwise turn 3% into 5%).
    fn sync_levels(&self) {
        let raw = levels::read_raw(&self.board);
        let mut l = self.levels.lock().unwrap();
        let mut moved = false;
        for (id, r) in raw {
            let cur = *l.get(&id).unwrap_or(&levels::DEFAULT);
            if levels::percent_to_raw(cur) != r {
                l.insert(id, levels::raw_to_percent(r));
                moved = true;
            }
        }
        if moved {
            levels::save(&l);
        }
    }

    async fn announce(self: &Arc<Self>, target: &str, source: &str) {
        let ids: Vec<String> = if target == "all" {
            self.board.outputs.iter().map(|p| p.id.clone()).collect()
        } else if self.board.output(target).is_some() {
            vec![target.to_string()]
        } else {
            eprintln!("audiod: cmd/audio/announce: no output '{target}'");
            return;
        };
        for id in ids {
            let (app, source) = (self.clone(), source.to_string());
            tokio::spawn(async move {
                let Some(turn) = app.announce_turn.get(&id) else { return };
                let _turn = turn.lock().await;
                let Some(p) = app.board.output(&id) else { return };
                app.announcing.lock().unwrap().insert(id.clone());
                app.changed.notify_one();
                if let Err(e) = announce::play(p, &source).await {
                    eprintln!("audiod: announce on {id}: {e}");
                }
                app.announcing.lock().unwrap().remove(&id);
                app.changed.notify_one();
            });
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
                // A route plays whenever it runs; an endpoint when it says so.
                "playing": up.get(&i.tag).copied().unwrap_or(false)
                    && (i.kind == "route" || meta::playing(&i.tag)),
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

#[utoipa::path(get, path = "/api/audio/cover/{tag}", tag = "Audio",
    summary = "An AirPlay endpoint's cover art",
    description = "The image the sender sent with the current track; `state/audio/meta/<tag>` names it (`cover`). Spotify covers are absolute URLs there instead.",
    params(("tag" = String, Path, description = "endpoint tag, e.g. airplay-1")),
    responses((status = 200, description = "the image"), (status = 404, description = "no cover art")))]
async fn get_cover(axum::extract::Path(tag): axum::extract::Path<String>) -> axum::response::Response {
    if !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match std::fs::read(meta::cover_path(&tag)) {
        Ok(b) => (
            [(axum::http::header::CONTENT_TYPE, meta::mime(&b)), (axum::http::header::CACHE_CONTROL, "max-age=86400")],
            b,
        ).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
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
        app.sync_levels();
        let levels = app.levels.lock().unwrap().clone();
        let announcing = app.announcing.lock().unwrap().clone();
        for p in &app.board.outputs {
            let level = *levels.get(&p.id).unwrap_or(&levels::DEFAULT);
            r.set(client, base, &format!("audio/level/{}", p.id), &json!(level)).await;
            r.set(client, base, &format!("audio/announcing/{}", p.id), &json!(announcing.contains(&p.id))).await;
        }
        // Now playing, per endpoint. Null (an empty retained message) clears it.
        let tags: Vec<String> = app.instances.lock().unwrap().iter()
            .filter(|i| i.kind == "spotify" || i.kind == "airplay")
            .map(|i| i.tag.clone())
            .collect();
        for tag in tags {
            let m = meta::load(&tag).map(|m| json!(m)).unwrap_or(Value::Null);
            r.set(client, base, &format!("audio/meta/{tag}"), &m).await;
        }
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
async fn command(app: &Arc<App>, what: &str, body: &str) {
    let body = body.trim();
    if let Some(id) = what.strip_prefix("level/") {
        match body.parse::<u8>() {
            Ok(p) => {
                if let Err(e) = app.set_level(id, p) {
                    eprintln!("audiod: cmd/audio/level: {e}");
                }
            }
            Err(_) => eprintln!("audiod: cmd/audio/level/{id}: not a percent: {body:?}"),
        }
        app.changed.notify_one();
        return;
    }
    if let Some(target) = what.strip_prefix("announce/") {
        app.announce(target, body).await;
        return;
    }
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
    // librespot's --onevent program (see meta.rs): handle one event and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--hook") {
        if let Some(tag) = args.get(2) {
            meta::hook(tag);
        }
        return;
    }
    let bind = std::env::var("AUDIOD_BIND").unwrap_or_else(|_| "0.0.0.0:7072".into());
    let board = Board::from_env();
    let endpoints = Endpoints::load();
    if board.has_map() {
        if let Err(e) = endpoints.validate(&board) {
            eprintln!("audiod: saved map invalid ({e}); endpoints that do not match the board are skipped");
        }
    }
    if board.has_map() {
        let _ = std::fs::create_dir_all("/run/ohc/audio");
        if let Err(e) = std::fs::write(announce::CHIME, announce::chime_wav(board.rate)) {
            eprintln!("audiod: cannot write {}: {e}", announce::CHIME);
        }
    }
    let announce_turn = board.outputs.iter().map(|p| (p.id.clone(), tokio::sync::Mutex::new(()))).collect();
    let app = Arc::new(App {
        board,
        endpoints: Mutex::new(endpoints),
        instances: Mutex::new(Vec::new()),
        up: Default::default(),
        levels: Mutex::new(levels::load()),
        announcing: Mutex::new(HashSet::new()),
        announce_turn,
        readers: Mutex::new(HashSet::new()),
        changed: Notify::new(),
    });

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    rt.block_on(async move {
        // Bind before starting anything: a second copy (a restart racing the
        // old one's exit) must fail here, before it has spawned endpoints that
        // process::exit would then orphan.
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("audiod: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        app.restart();
        eprintln!(
            "audiod: {} outputs, {} inputs, {} instances",
            app.board.outputs.len(),
            app.board.inputs.len(),
            app.instances.lock().unwrap().len()
        );
        tokio::spawn(mqtt(app.clone()));
        // A librespot event hook (meta::hook) pokes us to publish now.
        {
            let app = app.clone();
            tokio::spawn(async move {
                let Ok(mut usr1) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) else { return };
                while usr1.recv().await.is_some() {
                    app.changed.notify_one();
                }
            });
        }

        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(get_audio))
            .routes(routes!(put_endpoints))
            .routes(routes!(health))
            .routes(routes!(get_cover))
            .with_state(app.clone())
            .split_for_parts();
        let router = router.route(
            "/api/openapi.json",
            axum::routing::get(move || {
                let api = api.clone();
                async move { Json(api) }
            }),
        );
        eprintln!("audiod: REST on {bind}");
        // On SIGTERM (the init script's stop) or ^C, stop at once — not
        // axum's graceful shutdown, which waits out webd's keep-alive
        // connections — and drop every instance, whose processes are
        // kill_on_drop. Dying on the default SIGTERM action instead left
        // librespot and shairport-sync orphaned, holding their ports.
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            r = axum::serve(listener, router).into_future() => {
                if let Err(e) = r {
                    eprintln!("audiod: {e}");
                }
            }
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        app.instances.lock().unwrap().clear();
        // Let the aborted supervisors drop their children (kill_on_drop).
        tokio::time::sleep(Duration::from_millis(200)).await;
        eprintln!("audiod: stopped");
    });
}
