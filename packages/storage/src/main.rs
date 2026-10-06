//! ohc-storaged — external drives, mounted and shared.
//!
//! Plug a USB stick or disk (or, on the HC-800, an eSATA drive) into any openHC
//! board with USB and it is mounted under /media and shared over SMB, which
//! macOS (Finder → Network, or smb://<box>.local) and Windows (\\<box>.local or
//! \\<ip>) both open natively. On boards with audio, ohc-audiod's library
//! player plays music from the same mounts.
//!
//! * **MQTT** (live): `<base>/state/storage/volumes` — every mounted volume
//!   (name, filesystem, bus, size, used, mount point); `<base>/state/storage/share`
//!   — whether sharing is on, the login, guest access, and the address.
//!   Commands: `<base>/cmd/storage/eject/<id>` (unmount and unshare before
//!   pulling the drive), `<base>/cmd/storage/rescan`.
//! * **REST** (configuration, :7073, webd `/storage/`): `GET /api/storage`,
//!   `PUT /api/storage/share` — sharing on/off, login, password, guest; and the
//!   web UI's file browser, `/api/files…` — list, download, upload (streamed),
//!   new folder, rename, delete, confined to the mounted volumes (files.rs).
//!
//! Drives are noticed by a udev rule poking this daemon (SIGUSR1) and, as a
//! backstop, a scan every few seconds of /sys/block (see drives.rs for what
//! counts as external — never a board's internal disk).
mod drives;
mod files;
mod smb;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use drives::{Found, Volume};
use ohcmqtt::{Base, Retained};
use rumqttc::{AsyncClient, Event, Incoming, QoS};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Notify;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

#[derive(OpenApi)]
#[openapi(
    info(title = "ohc-storaged", description = "External drives (USB, eSATA): mounted under /media and shared over SMB. Share settings here; volumes and eject over MQTT (<base>/state/storage/*, <base>/cmd/storage/*)."),
    tags((name = "Storage", description = "Volumes and the SMB share."))
)]
struct ApiDoc;

/// A volume this daemon mounted.
struct Mounted {
    found: Found,
    name: String,
    read_only: bool,
}

struct App {
    host: String,
    ata_ports: Vec<String>,
    settings: Mutex<smb::Settings>,
    mounted: Mutex<HashMap<String, Mounted>>,
    /// Ejected volumes stay out until their drive is unplugged.
    ejected: Mutex<HashSet<String>>,
    /// Volumes that would not mount: not retried until replugged (a
    /// filesystem this kernel lacks would otherwise fail every few seconds).
    failed: Mutex<HashSet<String>>,
    changed: Notify,
    rescan: Notify,
}

impl App {
    /// Bring mounts and shares in line with the drives present.
    fn sync(&self) {
        let found = drives::scan(&self.ata_ports);
        let present: HashSet<String> = found.iter().map(|f| f.id.clone()).collect();
        let mut changed = false;
        {
            let mut m = self.mounted.lock().unwrap();
            let mut ejected = self.ejected.lock().unwrap();
            // Unplugged (or changed under us): unmount.
            let gone: Vec<String> = m
                .iter()
                .filter(|(id, v)| !present.contains(*id) || !found.iter().any(|f| &f.id == *id && f.dev == v.found.dev && f.fs == v.found.fs))
                .map(|(id, _)| id.clone())
                .collect();
            for id in gone {
                if let Some(v) = m.remove(&id) {
                    eprintln!("storaged: {} ({}) went away; unmounting", v.name, id);
                    drives::unmount(&format!("{}/{}", drives::MEDIA, v.name));
                    changed = true;
                }
            }
            ejected.retain(|id| present.contains(id));
            let mut failed = self.failed.lock().unwrap();
            failed.retain(|id| present.contains(id));
            // New: mount.
            for f in found {
                if m.contains_key(&f.id) || ejected.contains(&f.id) || failed.contains(&f.id) {
                    continue;
                }
                let taken: Vec<String> = m.values().map(|v| v.name.clone()).collect();
                let name = drives::share_name(&f.label, &f.id, &taken);
                match drives::mount(&f, &name) {
                    Ok(ro) => {
                        eprintln!("storaged: mounted {} ({}, {}) at {}/{name}{}", f.dev, f.fs, f.bus, drives::MEDIA, if ro { " read-only" } else { "" });
                        m.insert(f.id.clone(), Mounted { found: f, name, read_only: ro });
                        changed = true;
                    }
                    Err(e) => {
                        eprintln!("storaged: {e}; not retrying until it is plugged in again");
                        failed.insert(f.id.clone());
                    }
                }
            }
        }
        if changed {
            self.apply_shares();
            self.changed.notify_one();
        }
    }

    fn shares(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .mounted
            .lock()
            .unwrap()
            .values()
            .map(|m| (m.name.clone(), format!("{}/{}", drives::MEDIA, m.name)))
            .collect();
        v.sort();
        v
    }

    fn apply_shares(&self) {
        let s = self.settings.lock().unwrap().clone();
        smb::apply(&self.host, &s, &self.shares());
    }

    fn eject(&self, id: &str) -> Result<(), String> {
        let v = self.mounted.lock().unwrap().remove(id).ok_or_else(|| format!("no mounted volume '{id}'"))?;
        self.ejected.lock().unwrap().insert(id.to_string());
        self.apply_shares();
        // Flush before the unmount returns, so pulling the drive is safe.
        let _ = std::process::Command::new("sync").status();
        drives::unmount(&format!("{}/{}", drives::MEDIA, v.name));
        eprintln!("storaged: ejected {} ({id})", v.name);
        self.changed.notify_one();
        Ok(())
    }

    fn volumes(&self) -> Vec<Volume> {
        let mut v: Vec<Volume> = self
            .mounted
            .lock()
            .unwrap()
            .values()
            .map(|m| {
                let at = format!("{}/{}", drives::MEDIA, m.name);
                let (size, used) = drives::usage(&at).map_or((m.found.size_bytes, None), |(t, u)| (t, Some(u)));
                Volume {
                    id: m.found.id.clone(),
                    name: m.name.clone(),
                    label: m.found.label.clone(),
                    fs: m.found.fs.clone(),
                    bus: m.found.bus.clone(),
                    drive: m.found.drive.clone(),
                    size_bytes: size,
                    used_bytes: used,
                    mount: Some(at),
                    read_only: m.read_only,
                }
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Mounted volumes' names and whether each is read-only (for files.rs).
    fn volume_names(&self) -> Vec<(String, bool)> {
        self.mounted.lock().unwrap().values().map(|m| (m.name.clone(), m.read_only)).collect()
    }

    fn share_doc(&self) -> Value {
        let s = self.settings.lock().unwrap().clone();
        json!({
            "enabled": s.enabled,
            "user": s.user,
            "guest": s.guest,
            "host": format!("{}.local", self.host),
            "smb": format!("smb://{}.local", self.host),
            "windows": format!("\\\\{}.local", self.host),
        })
    }
}

type Ctx = State<Arc<App>>;

#[utoipa::path(get, path = "/api/storage", tag = "Storage",
    summary = "Mounted volumes and the share settings",
    description = "For drawing a page once — volumes come and go live on MQTT (`<base>/state/storage/volumes`).",
    responses((status = 200, description = "volumes and share")))]
async fn get_storage(State(app): Ctx) -> Json<Value> {
    let mut share = app.share_doc();
    // The generated first-start password, until one is set (REST only: the
    // retained MQTT state is readable by anything on the broker).
    if let Some(pw) = app.settings.lock().unwrap().initial_password.clone() {
        share["initial_password"] = json!(pw);
    }
    Json(json!({ "volumes": app.volumes(), "share": share }))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ShareUpdate {
    enabled: Option<bool>,
    user: Option<String>,
    /// A new password for the login (never returned).
    password: Option<String>,
    guest: Option<bool>,
}

#[utoipa::path(put, path = "/api/storage/share", tag = "Storage",
    summary = "Change the SMB share settings",
    description = "Any of: sharing on/off, the login name, its password, guest access. Saved persistently and applied at once.",
    request_body = ShareUpdate,
    responses((status = 200, description = "the share settings"), (status = 400, description = "invalid user name or password")))]
async fn put_share(State(app): Ctx, Json(u): Json<ShareUpdate>) -> axum::response::Response {
    let mut s = app.settings.lock().unwrap().clone();
    if let Some(user) = &u.user {
        if !smb::valid_user(user) {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "user: letters, digits, - and _ only" }))).into_response();
        }
        s.user = user.clone();
    }
    if let Some(e) = u.enabled {
        s.enabled = e;
    }
    if let Some(g) = u.guest {
        s.guest = g;
    }
    // A renamed login needs its password set again.
    let renamed = u.user.is_some() && u.user.as_deref() != Some(app.settings.lock().unwrap().user.as_str());
    match (&u.password, renamed) {
        (Some(p), _) => {
            if let Err(e) = smb::set_password(&s.user, p) {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
            }
            s.initial_password = None;
        }
        (None, true) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "a new login needs a password" }))).into_response();
        }
        _ => {}
    }
    if let Err(e) = smb::save(&s) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("save: {e}") }))).into_response();
    }
    *app.settings.lock().unwrap() = s;
    app.apply_shares();
    app.changed.notify_one();
    Json(app.share_doc()).into_response()
}

#[derive(Deserialize, utoipa::IntoParams)]
struct PathQuery {
    /// `<volume>/<path inside it>`, e.g. `Sandisk/Music/track.flac`.
    path: String,
}

#[utoipa::path(get, path = "/api/files", tag = "Storage",
    summary = "List a folder on a drive",
    description = "Folders first, then files: name, dir, size (bytes), modified (Unix seconds). `path` = `<volume>` lists a drive's top.",
    params(PathQuery),
    responses((status = 200, description = "entries", body = Vec<files::Entry>), (status = 404, description = "no such volume or folder")))]
async fn files_list(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<PathQuery>) -> axum::response::Response {
    match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), false) {
        Ok((p, _)) => match files::list(&p) {
            Ok(v) => Json(v).into_response(),
            Err(r) => r,
        },
        Err(r) => r,
    }
}

#[utoipa::path(get, path = "/api/files/download", tag = "Storage",
    summary = "Download a file from a drive",
    params(PathQuery),
    responses((status = 200, description = "the file, streamed, as an attachment"), (status = 404, description = "not found")))]
async fn files_download(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<PathQuery>) -> axum::response::Response {
    match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), false) {
        Ok((p, _)) if p.is_file() => files::download(&p).await,
        Ok(_) => files::err(StatusCode::BAD_REQUEST, "not a file"),
        Err(r) => r,
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
struct UploadQuery {
    /// Where the file goes: `<volume>/<folder…>/<file name>`.
    path: String,
    /// Replace a file of the same name (otherwise 409).
    #[serde(default)]
    overwrite: bool,
}

#[utoipa::path(put, path = "/api/files/upload", tag = "Storage",
    summary = "Upload a file to a drive",
    description = "The request body is the file's bytes (any size; streamed to the drive through a temporary file, renamed into place when complete).",
    params(UploadQuery),
    responses((status = 200, description = "stored"), (status = 403, description = "read-only volume"), (status = 409, description = "exists"), (status = 507, description = "drive full")))]
async fn files_upload(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<UploadQuery>, body: axum::body::Body) -> axum::response::Response {
    let (p, _) = match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !p.file_name().is_some_and(|n| files::valid_name(&n.to_string_lossy())) || !p.parent().is_some_and(|d| d.is_dir()) {
        return files::err(StatusCode::BAD_REQUEST, "upload into an existing folder, with a file name");
    }
    let r = files::upload(&p, body, q.overwrite).await;
    app.changed.notify_one();
    r
}

#[utoipa::path(post, path = "/api/files/mkdir", tag = "Storage",
    summary = "Make a folder on a drive",
    params(PathQuery),
    responses((status = 200, description = "made"), (status = 409, description = "exists")))]
async fn files_mkdir(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<PathQuery>) -> axum::response::Response {
    let (p, _) = match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !p.file_name().is_some_and(|n| files::valid_name(&n.to_string_lossy())) {
        return files::err(StatusCode::BAD_REQUEST, "bad folder name");
    }
    match std::fs::create_dir(&p) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => files::err(StatusCode::CONFLICT, "exists"),
        Err(e) => files::err(StatusCode::BAD_REQUEST, format!("{e}")),
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
struct RenameQuery {
    /// The file or folder: `<volume>/<path>`.
    path: String,
    /// Its new name, in the same folder.
    to: String,
}

#[utoipa::path(post, path = "/api/files/rename", tag = "Storage",
    summary = "Rename a file or folder on a drive",
    params(RenameQuery),
    responses((status = 200, description = "renamed"), (status = 409, description = "the new name exists")))]
async fn files_rename(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<RenameQuery>) -> axum::response::Response {
    let (p, vol) = match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if !files::valid_name(&q.to) || p == std::path::Path::new(drives::MEDIA).join(&vol) {
        return files::err(StatusCode::BAD_REQUEST, "bad name");
    }
    let to = p.with_file_name(&q.to);
    if to.exists() {
        return files::err(StatusCode::CONFLICT, "the new name exists");
    }
    match std::fs::rename(&p, &to) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => files::err(StatusCode::BAD_REQUEST, format!("{e}")),
    }
}

#[utoipa::path(delete, path = "/api/files", tag = "Storage",
    summary = "Delete a file or folder (with everything in it) from a drive",
    params(PathQuery),
    responses((status = 200, description = "deleted"), (status = 404, description = "not found")))]
async fn files_delete(State(app): Ctx, axum::extract::Query(q): axum::extract::Query<PathQuery>) -> axum::response::Response {
    let (p, vol) = match files::resolve(std::path::Path::new(drives::MEDIA), &q.path, &app.volume_names(), true) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if p == std::path::Path::new(drives::MEDIA).join(&vol) {
        return files::err(StatusCode::BAD_REQUEST, "that is the whole drive");
    }
    let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    app.changed.notify_one();
    match r {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => files::err(StatusCode::NOT_FOUND, format!("{e}")),
    }
}

#[utoipa::path(get, path = "/api/health", tag = "Storage", summary = "ohc-storaged liveness",
    responses((status = 200, description = "ok")))]
async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "ohc-storaged" }))
}

async fn mqtt(app: Arc<App>) {
    let base = Base::resolve();
    let (client, mut events) = AsyncClient::new(ohcmqtt::options("storaged", &base), 32);
    let reconnected = Arc::new(Notify::new());
    {
        let (app, client, base, reconnected) = (app.clone(), client.clone(), base.clone(), reconnected.clone());
        tokio::spawn(async move {
            let mut r = Retained::new();
            loop {
                tokio::select! {
                    // Used space changes while files are copied; a slow tick shows it.
                    _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                    _ = app.changed.notified() => {}
                    _ = reconnected.notified() => { r.republish(&client, &base).await; }
                }
                r.set(&client, &base, "storage/volumes", &json!(app.volumes())).await;
                r.set(&client, &base, "storage/share", &app.share_doc()).await;
            }
        });
    }
    let prefix = base.cmd("storage/");
    loop {
        match events.poll().await {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                ohcmqtt::online(&client, "storaged", &base).await;
                let _ = client.subscribe(base.cmd("storage/#"), QoS::AtLeastOnce).await;
                reconnected.notify_one();
            }
            Ok(Event::Incoming(Incoming::Publish(p))) => match p.topic.strip_prefix(&prefix) {
                Some("rescan") => app.rescan.notify_one(),
                Some(w) if w.starts_with("eject/") => {
                    if let Err(e) = app.eject(&w["eject/".len()..]) {
                        eprintln!("storaged: cmd/storage/{w}: {e}");
                    }
                }
                Some(other) => eprintln!("storaged: unknown command storage/{other}"),
                None => {}
            },
            Ok(_) => {}
            Err(e) => {
                eprintln!("storaged: mqtt: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

/// ksmbd.mountd, in the foreground, restarted if it exits.
async fn mountd() {
    loop {
        let r = Command::new("ksmbd.mountd")
            .args(["-n", "-C", &smb::conf().display().to_string(), "-P", &smb::pwddb().display().to_string()])
            .kill_on_drop(true)
            .status()
            .await;
        eprintln!("storaged: ksmbd.mountd exited ({r:?}); restarting in 3 s");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "openhc".into())
}

fn main() {
    let bind = std::env::var("STORAGED_BIND").unwrap_or_else(|_| "0.0.0.0:7073".into());
    let ata_ports = std::env::var("OHC_STORAGE_ATA").unwrap_or_default().split_whitespace().map(str::to_string).collect();
    let _ = std::fs::create_dir_all(smb::RUN);
    let _ = std::fs::create_dir_all(drives::MEDIA);
    let settings = smb::load();
    let app = Arc::new(App {
        host: hostname(),
        ata_ports,
        settings: Mutex::new(settings.clone()),
        mounted: Mutex::new(HashMap::new()),
        ejected: Mutex::new(HashSet::new()),
        failed: Mutex::new(HashSet::new()),
        changed: Notify::new(),
        rescan: Notify::new(),
    });
    // First start: the login gets a random password, shown on the web UI's
    // Storage page until it is changed there (or PUT /api/storage/share).
    app.apply_shares();
    if !smb::pwddb().exists() {
        match smb::random_password() {
            Ok(pw) => match smb::set_password(&settings.user, &pw) {
                Ok(()) => {
                    let mut s = app.settings.lock().unwrap();
                    s.initial_password = Some(pw);
                    if let Err(e) = smb::save(&s) {
                        eprintln!("storaged: cannot save the share settings: {e}");
                    }
                }
                Err(e) => eprintln!("storaged: cannot create the SMB login: {e}"),
            },
            Err(e) => eprintln!("storaged: no random source for the SMB password: {e}"),
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("storaged: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        tokio::spawn(mqtt(app.clone()));
        let mountd_task = tokio::spawn(mountd());

        // Drive scanning: on a udev poke (SIGUSR1), a rescan command, or every
        // few seconds as a backstop. Blocking work (mount, udevadm) runs off
        // the async thread.
        {
            let app = app.clone();
            tokio::spawn(async move {
                let Ok(mut usr1) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) else { return };
                loop {
                    let a = app.clone();
                    let _ = tokio::task::spawn_blocking(move || a.sync()).await;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                        _ = usr1.recv() => { tokio::time::sleep(Duration::from_millis(800)).await; }
                        _ = app.rescan.notified() => {}
                    }
                }
            });
        }

        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(get_storage))
            .routes(routes!(put_share))
            .routes(routes!(health))
            .routes(routes!(files_list, files_delete))
            .routes(routes!(files_download))
            .routes(routes!(files_upload))
            .routes(routes!(files_mkdir))
            .routes(routes!(files_rename))
            .with_state(app.clone())
            .split_for_parts();
        // Uploads are files of any size, streamed: no body limit.
        let router = router.layer(axum::extract::DefaultBodyLimit::disable());
        let router = router.route(
            "/api/openapi.json",
            axum::routing::get(move || {
                let api = api.clone();
                async move { Json(api) }
            }),
        );
        eprintln!("storaged: REST on {bind}");
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            r = axum::serve(listener, router).into_future() => {
                if let Err(e) = r {
                    eprintln!("storaged: {e}");
                }
            }
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        // Leave nothing half-written on a drive someone pulls after a stop.
        mountd_task.abort();
        smb::withdraw();
        let _ = std::process::Command::new("sync").status();
        let names: Vec<String> = app.mounted.lock().unwrap().values().map(|m| m.name.clone()).collect();
        for n in names {
            drives::unmount(&format!("{}/{n}", drives::MEDIA));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        eprintln!("storaged: stopped");
    });
}
