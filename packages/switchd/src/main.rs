//! switchd — the openHC managed-switch daemon.
//!
//! It owns the DSA switch ports (`lan1`, `lan2` on the EA3 / EA1-PoE BCM53125):
//! layer 2 (bridge membership, 802.1Q VLANs, admin state) and static layer 3.
//! By default the ports are one bridge — the box is a plain managed switch — and
//! an operator can split them into separate IP interfaces or VLANs over REST.
//!
//! SHAPE, per the owner's rules: MQTT carries what a control system or the web
//! UI watches change (per-port link/carrier/speed/stats, which bridge a port is
//! in, admin up/down commands); REST carries configuration (the topology, saved
//! as a serde file and re-applied on boot). webd reverse-proxies both under
//! `/switch`, so the browser sees one origin.
//!
//! "A service with nothing behind it does not appear": on a board with no
//! managed switch `OHC_DSA_PORTS` is empty, and switchd exits 0 at once. The
//! init script guards on the same variable, so this is belt-and-suspenders.
//!
//! Two ways to run:
//!   switchd apply   one-shot: load config, reconcile the kernel, write the
//!                   S40net handoff, exit. Run at boot (S39ea-switch) BEFORE
//!                   S40net leases, so the bridge exists first.
//!   switchd         daemon: apply once, then serve REST + MQTT and re-apply on
//!                   a config change.
mod apply;
mod config;
mod ports;

use axum::{extract::Path, extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::{Arc, Mutex};
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(OpenApi)]
#[openapi(
    info(title = "switchd", description = "Managed-switch control: bridge the DSA ports as one \
switch, or split them into separate IP interfaces and VLANs. Live per-port state is on MQTT; this \
REST surface is configuration."),
    tags(
        (name = "Switch", description = "Ports, bridge and VLAN topology of the managed switch.")
    )
)]
struct ApiDoc;

struct App {
    cfg: Mutex<config::SwitchConfig>,
    ports: Vec<String>,
}

/// The whole picture: the saved config plus the live state read back from the
/// kernel. The UI draws from this on load; live updates then arrive over MQTT.
fn overview_json(app: &App) -> serde_json::Value {
    let cfg = app.cfg.lock().unwrap();
    let port_status: Vec<_> = app.ports.iter().map(|p| ports::status(p)).collect();
    let br = &cfg.bridge;
    serde_json::json!({
        "mode": cfg.mode,
        "bridge": {
            "name": br.name,
            "present": ports::exists(&br.name),
            "stp": br.stp,
            "vlan_filtering": br.vlan_filtering,
            "members": br.members,
            "addresses": ports::addrs(&br.name),
        },
        "ports": port_status,
        "vlans": cfg.vlans,
        "config": *cfg,
    })
}

#[utoipa::path(get, path = "/api/switch", tag = "Switch",
    summary = "Switch overview: live port state + current topology",
    description = "The saved configuration together with what the kernel reports right now — \
carrier, speed, stats and bridge membership per port. `bridge.present` is false until DSA has \
actually built the bridge, which is the first thing to check if a port is not passing traffic.",
    responses((status = 200, description = "overview")))]
async fn overview(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(overview_json(&app))
}

#[utoipa::path(get, path = "/api/switch/config", tag = "Switch",
    summary = "The saved switch configuration",
    responses((status = 200, description = "config", body = config::SwitchConfig)))]
async fn get_config(State(app): State<Arc<App>>) -> Json<config::SwitchConfig> {
    Json(app.cfg.lock().unwrap().clone())
}

#[utoipa::path(put, path = "/api/switch/config", tag = "Switch",
    summary = "Replace the switch configuration and apply it",
    description = "Validates against this board's real ports first (a bridge member that is not a \
port, or managed mode with no members, is refused so a typo cannot strand the box), then persists \
to /etc/openhc/switch.json and reconciles the running network. Returns what ran.",
    request_body = config::SwitchConfig,
    responses(
        (status = 200, description = "applied", body = apply::ApplyResult),
        (status = 400, description = "invalid config")))]
async fn put_config(
    State(app): State<Arc<App>>,
    Json(new): Json<config::SwitchConfig>,
) -> impl IntoResponse {
    let errs = new.validate(&app.ports);
    if !errs.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "errors": errs }))).into_response();
    }
    if let Err(e) = config::save(&new) {
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "errors": [format!("save: {e}")] }))).into_response();
    }
    let result = apply::apply(&new, &app.ports);
    *app.cfg.lock().unwrap() = new;
    (StatusCode::OK, Json(result)).into_response()
}

#[utoipa::path(post, path = "/api/switch/apply", tag = "Switch",
    summary = "Re-apply the current configuration",
    description = "Reconcile the kernel to the saved config without changing it — useful after a \
port that was absent at boot shows up.",
    responses((status = 200, description = "applied", body = apply::ApplyResult)))]
async fn reapply(State(app): State<Arc<App>>) -> Json<apply::ApplyResult> {
    let cfg = app.cfg.lock().unwrap().clone();
    Json(apply::apply(&cfg, &app.ports))
}

#[utoipa::path(post, path = "/api/switch/port/{name}/enable", tag = "Switch",
    summary = "Admin-enable or disable one port (persisted)",
    description = "Sets `enabled` on the port and re-applies. A quick toggle; the full topology is \
the PUT above.",
    params(("name" = String, Path, description = "DSA port name, e.g. lan1")),
    request_body = inline(serde_json::Value),
    responses((status = 200, description = "applied"), (status = 404, description = "no such port")))]
async fn set_port_enable(
    State(app): State<Arc<App>>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !app.ports.iter().any(|p| p == &name) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "no such port" }))).into_response();
    }
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let cfg = {
        let mut c = app.cfg.lock().unwrap();
        c.ports.entry(name.clone()).or_default().enabled = enabled;
        c.clone()
    };
    let _ = config::save(&cfg);
    let result = apply::apply(&cfg, &app.ports);
    (StatusCode::OK, Json(result)).into_response()
}

#[utoipa::path(get, path = "/api/health", tag = "Switch",
    summary = "switchd liveness", responses((status = 200, description = "ok")))]
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "switchd" }))
}

fn main() {
    let ports = config::dsa_ports();
    // A board with no managed switch: nothing to own. Exit cleanly so neither
    // the boot nor the supervisor treats it as a failure.
    if ports.is_empty() {
        eprintln!("switchd: no OHC_DSA_PORTS — this board has no managed switch, nothing to do");
        return;
    }

    let cfg = config::load(&ports);

    // One-shot boot mode: reconcile and get out of the way of S40net.
    if std::env::args().nth(1).as_deref() == Some("apply") {
        let r = apply::apply(&cfg, &ports);
        for s in &r.steps {
            eprintln!("switchd: {s}");
        }
        for e in &r.errors {
            eprintln!("switchd: ERROR {e}");
        }
        eprintln!("switchd: apply {}", if r.ok { "ok" } else { "with errors (best-effort)" });
        return; // never fail boot
    }

    let app = Arc::new(App { cfg: Mutex::new(cfg.clone()), ports: ports.clone() });
    // 7074, not 7073: ohc-storaged took 7073 on main. webd's proxy maps /switch
    // to this (WEBD_SWITCHD_ADDR), so keep the two in step.
    let bind = std::env::var("SWITCHD_BIND").unwrap_or_else(|_| "0.0.0.0:7074".into());
    let period: u64 = std::env::var("SWITCHD_PERIOD").ok().and_then(|v| v.parse().ok()).unwrap_or(5);

    eprintln!("switchd: managing {} on {bind} (mode {:?})", ports.join(" "), cfg.mode);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    rt.block_on(async move {
        // Converge the kernel to the saved config on startup too: a reboot that
        // skipped S39ea-switch, or a daemon restart, still ends up correct.
        let _ = apply::apply(&app.cfg.lock().unwrap().clone(), &app.ports);

        let base = ohcmqtt::Base::resolve();
        let (client, events) = rumqttc::AsyncClient::new(ohcmqtt::options("switchd", &base), 16);
        let reconnected = Arc::new(tokio::sync::Notify::new());
        let changed = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(mqtt_loop(events, client.clone(), base.clone(), app.clone(),
                               reconnected.clone(), changed.clone()));

        // State publisher: every port's live status + the bridge, on a timer and
        // on demand (a command changed something).
        let pub_app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(period));
            let mut retained = ohcmqtt::Retained::new();
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = reconnected.notified() => { retained.republish(&client, &base).await; continue; }
                    _ = changed.notified() => {}
                }
                let (mode, bridge) = {
                    let cfg = pub_app.cfg.lock().unwrap();
                    (serde_json::json!(cfg.mode), serde_json::json!({
                        "name": cfg.bridge.name,
                        "present": ports::exists(&cfg.bridge.name),
                        "stp": cfg.bridge.stp,
                        "vlan_filtering": cfg.bridge.vlan_filtering,
                        "members": cfg.bridge.members,
                        "addresses": ports::addrs(&cfg.bridge.name),
                    }))
                };
                retained.set(&client, &base, "switch/mode", &mode).await;
                retained.set(&client, &base, "switch/bridge", &bridge).await;
                for p in &pub_app.ports {
                    let s = serde_json::to_value(ports::status(p)).unwrap_or_default();
                    retained.set(&client, &base, &format!("switch/port/{p}"), &s).await;
                }
            }
        });

        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(health))
            .routes(routes!(overview))
            .routes(routes!(get_config))
            .routes(routes!(put_config))
            .routes(routes!(reapply))
            .routes(routes!(set_port_enable))
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
            eprintln!("switchd: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        eprintln!("switchd: listening on {bind}");
        let shutdown = async { let _ = tokio::signal::ctrl_c().await; };
        if let Err(e) = axum::serve(listener, router).with_graceful_shutdown(shutdown).await {
            eprintln!("switchd: server error: {e}");
        }
    });
}

/// MQTT: announce online, take port admin commands, and ask the publisher to
/// refresh after a (re)connect or a command.
async fn mqtt_loop(
    mut events: rumqttc::EventLoop,
    client: rumqttc::AsyncClient,
    base: ohcmqtt::Base,
    app: Arc<App>,
    reconnected: Arc<tokio::sync::Notify>,
    changed: Arc<tokio::sync::Notify>,
) {
    use rumqttc::{Event, Incoming, QoS};
    // `<base>/cmd/switch/port/<name>` with payload `up`/`down` — a live toggle
    // (not persisted; the REST enable endpoint is the persistent one).
    let cmd_prefix = base.cmd("switch/port/");
    let sub = base.cmd("switch/port/+");
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                ohcmqtt::online(&client, "switchd", &base).await;
                let _ = client.subscribe(sub.clone(), QoS::AtLeastOnce).await;
                reconnected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(p))) if p.topic.starts_with(&cmd_prefix) => {
                let name = p.topic[cmd_prefix.len()..].to_string();
                let body = String::from_utf8_lossy(&p.payload).trim().to_ascii_lowercase();
                if !app.ports.iter().any(|x| x == &name) {
                    eprintln!("switchd: cmd for unknown port {name:?}");
                    continue;
                }
                let up = match body.as_str() {
                    "up" | "on" | "1" => true,
                    "down" | "off" | "0" => false,
                    other => { eprintln!("switchd: port cmd not up/down: {other:?}"); continue; }
                };
                let _ = std::process::Command::new("ip")
                    .args(["link", "set", &name, if up { "up" } else { "down" }])
                    .status();
                changed.notify_one();
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("switchd: mqtt: {e}; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }
}
