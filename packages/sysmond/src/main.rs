//! sysmond — the openHC telemetry daemon.
//!
//! Temperatures, fans, CPU, memory and uptime, sampled on a timer, kept in a
//! bounded ring, and served over REST.
//!
//! WHY THIS IS NOT ON MQTT, and not part of iod. MQTT carries openHC's IO
//! CONTROL surface: retained state for things an automation acts on, where
//! "what is it right now" is the whole question and every message is an event
//! somebody might want to trigger on. A board temperature every five seconds is
//! none of that — it is telemetry. Publishing it retained would churn the
//! broker with values nobody subscribes to, and retained topics cannot answer
//! the question telemetry is actually for, which is "what did it do over the
//! last hour".
//!
//! So: iod owns IO and speaks MQTT; sysmond owns metrics and speaks REST, with
//! history. Two daemons, two surfaces, each shaped like the thing it carries.
//!
//! LIVE STATE IS ON MQTT TOO (owner's rule: anything a control system or the
//! web UI watches change is MQTT; REST is for configuration and queries). Every
//! board runs mosquitto, and sysmond is its own client of it (ohcmqtt): after
//! each sample it publishes `<base>/state/health/now` (the latest sample,
//! /api/now's shape) and `health/fan`, and once a minute `health/history` — the
//! ring downsampled to a point a minute, so six hours is ~360 points rather than
//! the full ring. `<base>/cmd/health/fan` takes `auto` or a percent. The REST
//! surface below stays for queries (a full-resolution history window) and other
//! API consumers.
mod fan;
mod history;
mod sensors;

use axum::{extract::Query, extract::State, Json};
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
use std::sync::{Arc, Mutex};

/// Document metadata. The paths come from the handlers themselves.
#[derive(OpenApi)]
#[openapi(
    info(title = "sysmond", description = "Board telemetry: temperatures, fans, CPU, memory and uptime, with history."),
    tags(
        (name = "Telemetry", description = "Sensors and machine-wide figures, sampled on a timer."),
        (name = "Fan", description = "Fan presence, mode and manual control — EA family only.")
    )
)]
struct ApiDoc;

struct App {
    store: Mutex<history::Store>,
    period: u64,
}

#[derive(serde::Deserialize)]
struct Window {
    /// Seconds of history to return. Absent or 0 means everything held.
    #[serde(default)]
    seconds: u64,
}

/// The latest sample, with labels — what a dashboard shows without asking for
/// history.
#[utoipa::path(
    get, path = "/api/now", tag = "Telemetry",
    summary = "The latest telemetry sample",
    description = "`series` describes each sensor once and `values` is positional against it. \
Labels are the chip's own — CPUTIN, SYSTIN — because renaming them to 'cpu' and 'board' would \
claim knowledge of where the thermistors physically sit.",
    responses((status = 200, description = "One sample, with the series that describes it"))
)]
async fn now(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(now_json(&app.store.lock().unwrap()))
}

/// The latest sample with its labels — /api/now, and `<base>/state/health/now`.
fn now_json(st: &history::Store) -> serde_json::Value {
    let latest = st.latest();
    serde_json::json!({
        "at": latest.map(|s| s.at),
        "series": st.series,
        "values": latest.map(|s| s.v.clone()),
        "cpu": latest.and_then(|s| s.cpu),
        "load1": latest.and_then(|s| s.load1),
        "mem_used_pct": latest.and_then(|s| s.mem_used_pct),
        "uptime_s": sensors::uptime_secs(),
        "mem_total_kb": sensors::mem().map(|(t, _)| t),
    })
}

/// The ring, keeping every `step`th sample (newest always kept) — what goes
/// out as `<base>/state/health/history`.
fn history_json(st: &history::Store, period: u64, step: usize) -> serde_json::Value {
    let all = st.since(0);
    let n = all.len();
    let picked: Vec<_> = all
        .into_iter()
        .enumerate()
        .filter(|(i, _)| (n - 1 - i) % step.max(1) == 0)
        .map(|(_, s)| s)
        .collect();
    serde_json::json!({
        "period_s": period * step.max(1) as u64,
        "held": picked.len(),
        "series": st.series,
        "samples": picked,
    })
}

/// History. `series` is sent ONCE and the samples are positional against it —
/// see the note in history.rs about not repeating every label per sample.
#[utoipa::path(
    get, path = "/api/history", tag = "Telemetry",
    summary = "Telemetry history",
    description = "A bounded ring in RAM: the daemon's memory is the same after a month as after \
a minute. `series` is sent once and samples are positional, so a six-hour window does not repeat \
every label four thousand times.",
    params(("seconds" = Option<u64>, Query, description = "How far back to return. Absent or 0 means everything held.")),
    responses((status = 200, description = "period, capacity, series and samples"))
)]
async fn history_h(State(app): State<Arc<App>>, Query(w): Query<Window>) -> Json<serde_json::Value> {
    let st = app.store.lock().unwrap();
    let since = if w.seconds == 0 {
        0
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(w.seconds)
    };
    Json(serde_json::json!({
        "period_s": app.period,
        "held": st.len(),
        "capacity": st.capacity(),
        "series": st.series,
        "samples": st.since(since),
    }))
}

#[utoipa::path(
    get, path = "/api/health", tag = "Telemetry",
    summary = "sysmond liveness",
    responses((status = 200, description = "ok"))
)]
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "sysmond" }))
}

/// Fan status. `available` is false on boards with no fan helper (the UI then
/// shows no control, only the readings), true with `mode` (auto|manual) and the
/// fan's current duty `pct` otherwise.
#[utoipa::path(
    get, path = "/api/fan", tag = "Fan",
    summary = "Fan presence, mode and current duty",
    description = "Shells out to the board's ohc-fand helper, present only on boards with a \
controllable fan. The EA fan's pwm lives under /sys/class/pwm, not hwmon, so its duty comes from \
the helper rather than the telemetry series.",
    responses((status = 200, description = "{ available, mode, pct }"))
)]
async fn fan_status() -> Json<serde_json::Value> {
    Json(fan::status())
}

#[derive(serde::Deserialize)]
struct FanSet {
    /// Duty percent 0-100; clamped before the helper sees it.
    pct: i64,
}

/// Set a manual fan override that PERSISTS — the daemon's curve stops fighting
/// it. The thermal fail-safe still forces 100% on a critical temperature.
#[utoipa::path(
    post, path = "/api/fan/set", tag = "Fan",
    summary = "Hold the fan at a manual percent",
    request_body = inline(serde_json::Value),
    responses((status = 200, description = "{ ok, mode, pct } or { ok:false, error }"))
)]
async fn fan_set(Json(req): Json<FanSet>) -> Json<serde_json::Value> {
    Json(fan::set(req.pct))
}

/// Release the fan back to its automatic temperature curve.
#[utoipa::path(
    post, path = "/api/fan/auto", tag = "Fan",
    summary = "Return the fan to automatic control",
    responses((status = 200, description = "{ ok, mode } or { ok:false, error }"))
)]
async fn fan_auto() -> Json<serde_json::Value> {
    Json(fan::auto())
}

fn main() {
    let bind = std::env::var("SYSMOND_BIND").unwrap_or_else(|_| "0.0.0.0:7071".into());
    // Six hours at five seconds is ~4300 samples. Compact, so a few hundred kB.
    let period: u64 = env_num("SYSMOND_PERIOD", 5);
    let window: u64 = env_num("SYSMOND_WINDOW", 6 * 3600);

    let app = Arc::new(App {
        store: Mutex::new(history::Store::new(window, period)),
        period,
    });
    eprintln!(
        "sysmond: sampling every {period}s, holding {}h ({} samples)",
        window / 3600,
        app.store.lock().unwrap().capacity()
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    rt.block_on(async move {
        let collector = app.clone();
        // One history point a minute, whatever the sampling period.
        let step = (60 / period.max(1)).max(1) as usize;
        let base = ohcmqtt::Base::resolve();
        let (client, events) = rumqttc::AsyncClient::new(ohcmqtt::options("sysmond", &base), 16);
        let reconnected = std::sync::Arc::new(tokio::sync::Notify::new());
        let fan_changed = std::sync::Arc::new(tokio::sync::Notify::new());
        tokio::spawn(mqtt_loop(events, client.clone(), base.clone(), reconnected.clone(), fan_changed.clone()));
        tokio::spawn(async move {
            let mut cpu = sensors::Cpu::default();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(period));
            let mut n: usize = 0;
            let mut retained = ohcmqtt::Retained::new();
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = reconnected.notified() => {
                        retained.republish(&client, &base).await;
                        continue;
                    }
                    _ = fan_changed.notified() => {
                        retained.set(&client, &base, "health/fan", &fan::status()).await;
                        continue;
                    }
                }
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let r = sensors::readings();
                let (total, avail) = sensors::mem().unwrap_or((0, 0));
                let mem_pct = (total > 0).then(|| ((total - avail) * 100 / total) as u8);
                let (now, hist) = {
                    let mut st = collector.store.lock().unwrap();
                    st.push(at, &r, cpu.sample(), sensors::load1(), mem_pct);
                    (now_json(&st), (n % step == 0).then(|| history_json(&st, period, step)))
                };
                retained.set(&client, &base, "health/now", &now).await;
                if let Some(h) = hist {
                    retained.set(&client, &base, "health/history", &h).await;
                }
                retained.set(&client, &base, "health/fan", &fan::status()).await;
                n = n.wrapping_add(1);
            }
        });

        // The ROUTER AND THE SPEC ARE THE SAME DECLARATION. routes!() reads the
        // #[utoipa::path] attribute on each handler for both the method/path it
        // mounts and the documentation it emits, so a route cannot exist
        // undocumented and a documented route cannot fail to exist.
        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(health))
            .routes(routes!(now))
            .routes(routes!(history_h))
            .routes(routes!(fan_status))
            .routes(routes!(fan_set))
            .routes(routes!(fan_auto))
            .with_state(app)
            .split_for_parts();
        // Served so webd can merge it — see webd's /api/openapi.json.
        let router = router.route(
            "/api/openapi.json",
            axum::routing::get(move || {
                let api = api.clone();
                async move { Json(api) }
            }),
        );
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("sysmond: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        eprintln!("sysmond: listening on {bind}");
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        if let Err(e) = axum::serve(listener, router).with_graceful_shutdown(shutdown).await {
            eprintln!("sysmond: server error: {e}");
        }
    });
}

/// The MQTT connection: online status, the fan command, and telling the
/// collector to republish after a reconnect.
async fn mqtt_loop(
    mut events: rumqttc::EventLoop,
    client: rumqttc::AsyncClient,
    base: ohcmqtt::Base,
    reconnected: std::sync::Arc<tokio::sync::Notify>,
    fan_changed: std::sync::Arc<tokio::sync::Notify>,
) {
    use rumqttc::{Event, Incoming, QoS};
    let fan_topic = base.cmd("health/fan");
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                ohcmqtt::online(&client, "sysmond", &base).await;
                let _ = client.subscribe(fan_topic.clone(), QoS::AtLeastOnce).await;
                reconnected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(p))) if p.topic == fan_topic => {
                let body = String::from_utf8_lossy(&p.payload).trim().to_ascii_lowercase();
                let r = if body == "auto" {
                    fan::auto()
                } else if let Ok(pct) = body.parse::<i64>() {
                    fan::set(pct)
                } else {
                    serde_json::json!({ "ok": false, "error": format!("not auto or a percent: {body:?}") })
                };
                if r.get("ok") == Some(&serde_json::json!(false)) {
                    eprintln!("sysmond: cmd/health/fan: {r}");
                }
                fan_changed.notify_one();
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("sysmond: mqtt: {e}; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }
}

fn env_num(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
