//! updated — openHC updates, run on the box.
//!
//! A channel — `stable` (the latest published GitHub release) or `dev` (the
//! rolling pre-release images.yml refreshes from main) — is checked against what
//! this box runs (/etc/openhc-release). Installing downloads the board's image
//! bundle, hands it to the board's own installer (install.rs) and reboots.
//!
//! * **MQTT** (live state and control, on the box's broker):
//!   `<base>/state/system/update` — JSON: channel, board, current, latest,
//!   available, installable, state (idle|checking|downloading|installing|
//!   rebooting|error), progress (0-100 while downloading), error, checked_at.
//!   Commands: `<base>/cmd/system/update/check`, `…/install`,
//!   `…/channel` (stable|dev).
//! * **REST** (loopback :7074, webd `/update/`): `GET /api/update`, `PUT
//!   /api/update/channel`, `POST /api/update/check`, `POST /api/update/install`.
//!
//! The channel is kept in /data/ohc/update.json. A check runs a minute after
//! start and every 12 hours; installing is only ever on request.
mod install;
mod release;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use ohcmqtt::{Base, Retained};
use release::{Asset, Channel};
use rumqttc::{AsyncClient, Event, Incoming, QoS};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::future::IntoFuture;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use utoipa_axum::{router::OpenApiRouter, routes};

const AGENT: &str = concat!("openhc-updated/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Settings {
    channel: Channel,
}

fn data_dir() -> PathBuf {
    let d = PathBuf::from(std::env::var("OHC_DATA").unwrap_or_else(|_| "/data".into())).join("ohc");
    if d.is_dir() { d } else { PathBuf::from("/run/ohc") }
}
fn settings_path() -> PathBuf {
    data_dir().join("update.json")
}

/// What this box is running, from /etc/openhc-release (post-build.sh).
fn this_build() -> (String, String) {
    let t = std::fs::read_to_string("/etc/openhc-release").unwrap_or_default();
    let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("unknown").trim().to_string();
    (get("board"), get("version"))
}

#[derive(Serialize, Clone, utoipa::ToSchema)]
struct Status {
    channel: Channel,
    board: String,
    /// This box's version.
    current: String,
    /// The channel's version for this board, once checked.
    latest: Option<String>,
    /// `latest` is set and differs from `current`.
    available: bool,
    /// This board has an on-box installer (others: use ohc-flash).
    installable: bool,
    /// idle | checking | downloading | installing | rebooting | error
    state: String,
    /// 0-100 while downloading.
    progress: Option<u8>,
    error: Option<String>,
    /// Unix seconds of the last successful check.
    checked_at: Option<u64>,
    #[serde(skip)]
    asset: Option<Asset>,
}

struct App {
    status: Mutex<Status>,
    changed: Notify,
}

impl App {
    fn snapshot(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    fn update(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
        self.changed.notify_one();
    }
    fn busy(&self) -> bool {
        matches!(self.status.lock().unwrap().state.as_str(), "checking" | "downloading" | "installing" | "rebooting")
    }

    fn set_channel(&self, c: Channel) {
        let s = Settings { channel: c };
        if let Ok(b) = serde_json::to_vec_pretty(&s) {
            let _ = std::fs::write(settings_path(), b);
        }
        self.update(|st| {
            st.channel = c;
            st.latest = None;
            st.available = false;
            st.asset = None;
        });
    }

    async fn check(self: &Arc<Self>) -> Result<(), String> {
        if self.busy() {
            return Err("busy".into());
        }
        let (channel, board) = {
            let s = self.status.lock().unwrap();
            (s.channel, s.board.clone())
        };
        self.update(|s| {
            s.state = "checking".into();
            s.error = None;
        });
        let r = tokio::task::spawn_blocking(move || -> Result<Option<(Asset, String)>, String> {
            let resp = ureq::get(&channel.api_url())
                .set("User-Agent", AGENT)
                .set("Accept", "application/vnd.github+json")
                .timeout(Duration::from_secs(30))
                .call();
            let rel: release::GhRelease = match resp {
                Ok(r) => r.into_json().map_err(|e| format!("GitHub: {e}"))?,
                // No release on that channel yet (no stable release published).
                Err(ureq::Error::Status(404, _)) => return Ok(None),
                Err(e) => return Err(format!("GitHub: {e}")),
            };
            Ok(release::pick(&rel, &board))
        })
        .await
        .map_err(|e| e.to_string())?;
        match r {
            Ok(found) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).ok();
                self.update(|s| {
                    s.available = found.as_ref().is_some_and(|(_, v)| *v != s.current);
                    s.latest = found.as_ref().map(|(_, v)| v.clone());
                    s.asset = found.map(|(a, _)| a);
                    s.state = "idle".into();
                    s.checked_at = now;
                });
                Ok(())
            }
            Err(e) => {
                self.update(|s| {
                    s.state = "error".into();
                    s.error = Some(e.clone());
                });
                Err(e)
            }
        }
    }

    /// Download the checked image, install it, reboot. Runs in the background;
    /// progress and the outcome are in the status.
    fn install(self: &Arc<Self>) -> Result<(), String> {
        if self.busy() {
            return Err("busy".into());
        }
        let (asset, board, available) = {
            let s = self.status.lock().unwrap();
            (s.asset.clone(), s.board.clone(), s.available)
        };
        let Some(asset) = asset else { return Err("check first: nothing to install".into()) };
        if !available {
            return Err("this box already runs that version".into());
        }
        if !install::supported(&board) {
            return Err(format!("no on-box installer for {board} yet — install with ohc-flash"));
        }
        self.update(|s| {
            s.state = "downloading".into();
            s.progress = Some(0);
            s.error = None;
        });
        let app = self.clone();
        std::thread::spawn(move || {
            let r = download(&app, &asset).and_then(|path| {
                app.update(|s| {
                    s.state = "installing".into();
                    s.progress = None;
                });
                let r = install::install(&board, &path);
                let _ = std::fs::remove_file(&path);
                r
            });
            match r {
                Ok(()) => {
                    app.update(|s| s.state = "rebooting".into());
                    std::thread::sleep(Duration::from_secs(3));
                    let _ = std::process::Command::new("reboot").status();
                }
                Err(e) => app.update(|s| {
                    s.state = "error".into();
                    s.progress = None;
                    s.error = Some(e);
                }),
            }
        });
        Ok(())
    }
}

/// The bundle into the data directory (a few hundred MB at most), with progress.
fn download(app: &App, asset: &Asset) -> Result<PathBuf, String> {
    let dir = data_dir().join("update");
    let _ = std::fs::remove_dir_all(&dir); // a stale partial from an earlier attempt
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(&asset.name);
    // A connect timeout but no overall one: the bundle can take minutes.
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(30)).build();
    let resp = agent
        .get(&asset.url)
        .set("User-Agent", AGENT)
        .call()
        .map_err(|e| format!("download: {e}"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(asset.size);
    let mut rd = resp.into_reader();
    let mut f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut got, mut shown) = (0u64, 0u8);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = rd.read(&mut buf).map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", path.display()))?;
        got += n as u64;
        if let Some(pct) = (got * 100).checked_div(total).map(|p| p.min(100) as u8) {
            if pct != shown {
                shown = pct;
                app.update(|s| s.progress = Some(pct));
            }
        }
    }
    f.sync_all().map_err(|e| e.to_string())?;
    if total > 0 && got != total {
        return Err(format!("download: got {got} of {total} bytes"));
    }
    Ok(path)
}

// ── REST ─────────────────────────────────────────────────────────────────────

#[derive(utoipa::OpenApi)]
#[openapi(info(title = "updated"), tags((name = "Updates", description = "Software updates from GitHub releases.")))]
struct ApiDoc;

fn err(code: StatusCode, e: &str) -> axum::response::Response {
    (code, Json(json!({ "error": e }))).into_response()
}

#[utoipa::path(get, path = "/api/update", tag = "Updates", summary = "Update status",
    responses((status = 200, body = Status)))]
async fn get_status(State(app): State<Arc<App>>) -> Json<Status> {
    Json(app.snapshot())
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ChannelBody {
    channel: Channel,
}
#[utoipa::path(put, path = "/api/update/channel", tag = "Updates", summary = "Choose the channel",
    description = "stable: published releases. dev: the rolling pre-release of main's latest build.",
    request_body = ChannelBody, responses((status = 200, body = Status)))]
async fn put_channel(State(app): State<Arc<App>>, Json(b): Json<ChannelBody>) -> axum::response::Response {
    if app.busy() {
        return err(StatusCode::CONFLICT, "busy");
    }
    app.set_channel(b.channel);
    let a = app.clone();
    tokio::spawn(async move { let _ = a.check().await; });
    Json(app.snapshot()).into_response()
}

#[utoipa::path(post, path = "/api/update/check", tag = "Updates", summary = "Check the channel now",
    responses((status = 200, body = Status), (status = 502, description = "GitHub unreachable")))]
async fn post_check(State(app): State<Arc<App>>) -> axum::response::Response {
    match app.check().await {
        Ok(()) => Json(app.snapshot()).into_response(),
        Err(e) if e == "busy" => err(StatusCode::CONFLICT, &e),
        Err(e) => err(StatusCode::BAD_GATEWAY, &e),
    }
}

#[utoipa::path(post, path = "/api/update/install", tag = "Updates", summary = "Install the checked version",
    description = "Downloads, installs and reboots, in the background — follow `state`. Refused when \
nothing newer was found, when busy, or on a board without an on-box installer.",
    responses((status = 202, body = Status), (status = 409, description = "refused")))]
async fn post_install(State(app): State<Arc<App>>) -> axum::response::Response {
    match app.install() {
        Ok(()) => (StatusCode::ACCEPTED, Json(app.snapshot())).into_response(),
        Err(e) => err(StatusCode::CONFLICT, &e),
    }
}

#[utoipa::path(get, path = "/api/health", tag = "Updates", summary = "updated liveness",
    responses((status = 200, description = "ok")))]
async fn health() -> &'static str {
    "ok"
}

// ── MQTT ─────────────────────────────────────────────────────────────────────

async fn mqtt(app: Arc<App>) {
    let base = Base::resolve();
    let (client, mut events) = AsyncClient::new(ohcmqtt::options("updated", &base), 16);
    let reconnected = Arc::new(Notify::new());
    {
        let (app, client, base, reconnected) = (app.clone(), client.clone(), base.clone(), reconnected.clone());
        tokio::spawn(async move {
            let mut r = Retained::new();
            loop {
                r.set(&client, &base, "system/update", &json!(app.snapshot())).await;
                tokio::select! {
                    _ = app.changed.notified() => {}
                    _ = reconnected.notified() => { r.republish(&client, &base).await; }
                }
            }
        });
    }
    let prefix = base.cmd("system/update/");
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                ohcmqtt::online(&client, "updated", &base).await;
                let _ = client.subscribe(base.cmd("system/update/#"), QoS::AtLeastOnce).await;
                reconnected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(p))) => {
                let body = String::from_utf8_lossy(&p.payload).to_string();
                match p.topic.strip_prefix(&prefix) {
                    Some("check") => {
                        let a = app.clone();
                        tokio::spawn(async move {
                            if let Err(e) = a.check().await {
                                eprintln!("updated: check: {e}");
                            }
                        });
                    }
                    Some("install") => {
                        if let Err(e) = app.install() {
                            eprintln!("updated: install: {e}");
                        }
                    }
                    Some("channel") => match Channel::parse(&body) {
                        Some(c) if !app.busy() => {
                            app.set_channel(c);
                            let a = app.clone();
                            tokio::spawn(async move { let _ = a.check().await; });
                        }
                        Some(_) => eprintln!("updated: channel: busy"),
                        None => eprintln!("updated: channel: expected stable or dev, got {body:?}"),
                    },
                    Some(other) => eprintln!("updated: unknown command system/update/{other}"),
                    None => {}
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("updated: mqtt: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

fn main() {
    let settings: Settings = std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let (board, current) = this_build();
    let app = Arc::new(App {
        status: Mutex::new(Status {
            channel: settings.channel,
            installable: install::supported(&board),
            board,
            current,
            latest: None,
            available: false,
            state: "idle".into(),
            progress: None,
            error: None,
            checked_at: None,
            asset: None,
        }),
        changed: Notify::new(),
    });
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    rt.block_on(async move {
        let bind = std::env::var("UPDATED_BIND").unwrap_or_else(|_| "127.0.0.1:7074".into());
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("updated: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        tokio::spawn(mqtt(app.clone()));
        {
            let app = app.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                loop {
                    if let Err(e) = app.check().await {
                        eprintln!("updated: check: {e}");
                    }
                    tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
                }
            });
        }
        let (router, api) = OpenApiRouter::with_openapi(<ApiDoc as utoipa::OpenApi>::openapi())
            .routes(routes!(get_status))
            .routes(routes!(put_channel))
            .routes(routes!(post_check))
            .routes(routes!(post_install))
            .routes(routes!(health))
            .split_for_parts();
        let router = router
            .route("/api/openapi.json", axum::routing::get(move || async move { Json(api) }))
            .with_state(app);
        eprintln!("updated: REST on {bind}");
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
        tokio::select! {
            r = axum::serve(listener, router).into_future() => { if let Err(e) = r { eprintln!("updated: {e}"); } }
            _ = term.recv() => {}
        }
    });
}
