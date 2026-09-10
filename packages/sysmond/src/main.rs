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
    tags((name = "Telemetry", description = "Sensors and machine-wide figures, sampled on a timer."))
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
    let st = app.store.lock().unwrap();
    let latest = st.latest();
    Json(serde_json::json!({
        "at": latest.map(|s| s.at),
        "series": st.series,
        "values": latest.map(|s| s.v.clone()),
        "cpu": latest.and_then(|s| s.cpu),
        "load1": latest.and_then(|s| s.load1),
        "mem_used_pct": latest.and_then(|s| s.mem_used_pct),
        "uptime_s": sensors::uptime_secs(),
        "mem_total_kb": sensors::mem().map(|(t, _)| t),
    }))
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
        tokio::spawn(async move {
            let mut cpu = sensors::Cpu::default();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(period));
            loop {
                tick.tick().await;
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let r = sensors::readings();
                let (total, avail) = sensors::mem().unwrap_or((0, 0));
                let mem_pct = (total > 0).then(|| ((total - avail) * 100 / total) as u8);
                collector
                    .store
                    .lock()
                    .unwrap()
                    .push(at, &r, cpu.sample(), sensors::load1(), mem_pct);
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

fn env_num(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
