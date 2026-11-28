//! openHC flasher — the desktop front end.
//!
//! Two tools in one window. By default it is a four-step wizard for someone who
//! has never opened a terminal: find the controller, check what it is, pick the
//! release, install. Flip **Details** and the same window becomes the debug
//! tool — the raw evidence the board was identified from, the exact flash
//! offsets that will be written, why the other install methods were rejected,
//! and every event the engine emits including the sub-lines the wizard hides.
//!
//! Concurrency is deliberately one lock. The engine is blocking and egui is
//! immediate-mode, so background threads write into a single [`Shared`] and the
//! UI renders whatever is in there each frame. A channel plus a drain loop plus
//! a state machine would be more code for the same result.

#![windows_subsystem = "windows"] // no console window behind the app on Windows

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use ohc_flash_core::board::Running;
use ohc_flash_core::board::Family;
use ohc_flash_core::{board, image, method, Board, Identity, Method};
use ohc_flash_engine::{choice, control4, hc250, hc800, iox, network, updates, Event, GhRelease, Install, Progress, Release, Zigbee};
use ohc_flash_transport as tp;

/// This flasher's own version, baked in by CI (`OHC_VERSION`) so it can tell
/// whether it is itself up to date. A local `cargo build` has none, and shows
/// a `-local` suffix that never matches a release — so a dev build is never
/// mistaken for the current release.
const VERSION: &str = match option_env!("OHC_VERSION") {
    Some(v) => v,
    None => concat!(env!("CARGO_PKG_VERSION"), "-local"),
};

/// How long to wait for the controller to come back after the stage-1 reboot.
/// Same budget the CLI uses; it is a real deadline, so the UI can show a real
/// progress bar for it rather than an indeterminate spinner.
const REBOOT_WAIT: u64 = 300;

const OK: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
const BAD: Color32 = Color32::from_rgb(0xd2, 0x3f, 0x31);
const WARN: Color32 = Color32::from_rgb(0xc9, 0x7c, 0x10);

/// The window/taskbar icon, drawn in code so the binary stays a single file with
/// no image assets to decode. A rounded square in the openHC green with a white
/// "HC" monogram — the mark of openHomeController, legible down to 16 px.
fn app_icon() -> egui::IconData {
    const N: usize = 128;
    // 5x7 block glyphs, one bit per pixel, MSB-left. Only the two letters the
    // monogram needs; a full font would be dead weight.
    const H: [u8; 7] = [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001];
    const C: [u8; 7] = [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110];

    let mut px = vec![0u8; N * N * 4];
    let put = |px: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
        let i = (y * N + x) * 4;
        px[i..i + 4].copy_from_slice(&c);
    };

    // Rounded-square background: openHC green, corners clipped to a radius.
    let (bg, ink) = ([0x2e, 0xa0, 0x43, 0xff], [0xff, 0xff, 0xff, 0xff]);
    let r = 22i32;
    for y in 0..N {
        for x in 0..N {
            let (xi, yi) = (x as i32, y as i32);
            let inside_x = xi.min(N as i32 - 1 - xi);
            let inside_y = yi.min(N as i32 - 1 - yi);
            let corner = inside_x < r && inside_y < r;
            let clipped = corner
                && ((r - inside_x).pow(2) + (r - inside_y).pow(2)) > r * r;
            if !clipped {
                put(&mut px, x, y, bg);
            }
        }
    }

    // "HC", each glyph a 5x7 grid scaled up, laid side by side and centred.
    let scale = 10usize;
    let gap = scale; // one cell between the letters
    let glyph_w = 5 * scale;
    let total_w = glyph_w * 2 + gap;
    let x0 = (N - total_w) / 2;
    let y0 = (N - 7 * scale) / 2;
    for (gi, glyph) in [H, C].iter().enumerate() {
        let gx = x0 + gi * (glyph_w + gap);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(&mut px, gx + col * scale + dx, y0 + row * scale + dy, ink);
                        }
                    }
                }
            }
        }
    }

    egui::IconData { rgba: px, width: N as u32, height: N as u32 }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 600.0])
            .with_min_inner_size([620.0, 460.0])
            .with_title("openHC Flasher")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "openHC Flasher",
        options,
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
}

// ---------------------------------------------------------------- shared state

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lvl {
    Step,
    Detail,
    Warn,
    /// Live console evidence while waiting on the user. Rendered dim and small:
    /// it exists to prove the tool is still watching, not to be read.
    Observed,
    /// A command this tool typed at the box's shell (the `shell` channel).
    Shell,
    /// BOOTP/TFTP server activity (the `net` channel) — debug noise, gated
    /// behind the details toggle like `Detail`.
    Net,
    /// A physical step the user completed.
    Resolved,
}

/// Where an install has got to. The three working phases are what the user is
/// told; `Restarting` is the only one with a real deadline behind it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Writing,
    Restarting,
    Finishing,
    /// openHC is in; now the Control4 extension (openhc-control4).
    Control4,
    Done,
    Failed,
}

/// A unit found on the network, plus whatever a login could tell us about it.
///
/// The list appears the moment the scan finishes and the extra fields fill in a
/// few seconds later, so the user is never staring at a spinner wondering
/// whether anything was found.
struct Unit {
    ip: String,
    mac: Option<String>,
    /// Answered Control4's SDDP director search — a positive controller id
    /// that holds even before we have a login.
    sddp: bool,
    hostname: Option<String>,
    running: Option<Running>,
    board: Option<&'static Board>,
    /// Name and model as the unit ANNOUNCED them, over SDDP.
    ///
    /// These arrive with no credentials at all, which is the point: a
    /// controller on OS 3.1 or later has no default root password, so the SSH
    /// probe below learns nothing about it and the row used to sit there saying
    /// "unknown". The unit had been telling us `Living-EA1 / C4-EA1` the whole
    /// time and we were discarding it.
    sddp_name: Option<String>,
    sddp_model: Option<String>,
    /// The OS version, once a login has read it. Control4 keeps it in a dpkg
    /// package, not a file — see `probe::version_of`.
    version: Option<String>,
    /// The probe has finished, whether or not it learned anything.
    probed: bool,
    /// Why it learned nothing.
    note: Option<String>,
}

impl Unit {
    /// What this unit is running, in plain language.
    fn firmware(&self) -> String {
        if !self.probed {
            return "checking…".into();
        }
        match self.running {
            // The version is the thing an installer actually wants to see: it
            // says which Control4 OS this is, and therefore whether a default
            // root password exists at all.
            Some(r) => match &self.version {
                Some(v) => format!("{} {v}", running_text(r)),
                None => running_text(r).to_string(),
            },
            // No login. Say what the unit announced rather than "unknown" — on
            // OS 3.1+ there is no password to be had, so this is not a step on
            // the way to knowing more, it is as much as there is.
            None if self.sddp_model.is_some() => format!(
                "{} — announced over SDDP; no login (OS 3.1+ has no root password)",
                self.sddp_model.clone().unwrap_or_default()
            ),
            None if self.sddp => "Control4 controller — no login yet".into(),
            None => self.note.clone().unwrap_or_else(|| "unknown".into()),
        }
    }

    /// The best name we have: what the unit calls itself over SDDP, else its
    /// hostname from a login, else nothing.
    fn display_name(&self) -> String {
        self.sddp_name
            .clone()
            .or_else(|| self.hostname.clone())
            .unwrap_or_default()
    }
}

/// A connected, identified controller.
struct Conn {
    host: String,
    ssh: tp::Ssh,
    id: Identity,
    /// The openHC version the box reports (`/etc/openhc-release`), if it runs
    /// openHC at all. `None` on stock Control4.
    installed_version: Option<String>,
}

/// A release that has been read and checked. `problems` empty means flashable.
struct RelInfo {
    source: String,
    kernel_len: u64,
    /// This release is for the HC-800, which has no CEFDK size window — so the
    /// "spare" figure the EA path shows would be a meaningless negative number.
    hc800: bool,
    problems: Vec<String>,
    names: Vec<String>,
}

/// Everything the background threads produce and the UI renders.
#[derive(Default)]
struct Shared {
    /// What is running right now, `None` when idle. Also gates every button.
    busy: Option<String>,
    log: Vec<(Lvl, String)>,
    found: Vec<Unit>,
    scanned: bool,
    conn: Option<Conn>,
    connect_error: Option<String>,
    rel: Option<Arc<Release>>,
    rel_info: Option<RelInfo>,
    rel_error: Option<String>,
    phase: Option<Phase>,
    error: Option<String>,
    /// (started, deadline) while waiting for the controller to reboot.
    wait: Option<(Instant, Instant)>,
    /// Stage 1 finished, so a failure afterwards is resumable with stage 2
    /// alone. Without this a timeout strands a half-installed unit.
    stage1_done: bool,
    /// The latest GitHub release, fetched once in the background. Feeds the
    /// "update available" hints and the "download latest image" button.
    latest: Option<GhRelease>,
    latest_error: Option<String>,
    checked_latest: bool,
    /// A one-line banner (e.g. the result of a self-update).
    notice: Option<String>,
    /// A physical action the tool is blocked on: (title, instruction). Sticky
    /// and rendered as a banner rather than a log line, because the user is
    /// looking at the hardware and would scroll straight past it.
    awaiting: Option<(String, String)>,
    /// A release image just downloaded, waiting for the UI thread to open it
    /// (set off-thread, consumed in `render`, same pattern as a dropped file).
    downloaded_image: Option<PathBuf>,
    /// The newest openHC and Home Assistant image releases (a published
    /// release, else the rolling dev one), for which choices have an image.
    img_openhc: Option<GhRelease>,
    img_ha: Option<GhRelease>,
    /// Which image family (`Install::image`) the loaded release was downloaded
    /// for, so switching between choices that share an image fetches nothing.
    loaded_image: Option<(&'static str, &'static str)>,
    /// Where the Control4 driver was saved, once the extension is installed.
    control4_driver: Option<PathBuf>,
    /// Why the Control4 step did not finish, when openHC itself did.
    control4_error: Option<String>,
}

impl Shared {
    fn push(&mut self, lvl: Lvl, text: String) {
        self.log.push((lvl, text));
    }
}

/// Start a background job. `busy` is set through the guard the caller already
/// holds — locking again here would be a deadlock, since the UI holds the lock
/// for the whole frame.
fn spawn(
    sh: &mut Shared,
    shared: &Arc<Mutex<Shared>>,
    what: &str,
    job: impl FnOnce(&Arc<Mutex<Shared>>) + Send + 'static,
) {
    sh.busy = Some(what.to_string());
    let s = Arc::clone(shared);
    std::thread::spawn(move || {
        job(&s);
        s.lock().unwrap().busy = None;
    });
}

/// An engine progress sink that appends to the shared log.
fn sink(shared: &Arc<Mutex<Shared>>) -> Progress {
    let s = Arc::clone(shared);
    Progress::new(move |e| {
        // Set the banner before logging, and clear it on anything that means
        // the wait is over -- Event::clears_await() owns that rule so the GUI
        // does not have to track the variant list.
        let mut g = s.lock().unwrap();
        if let Event::Await { title, instruction } = &e {
            g.awaiting = Some((title.clone(), instruction.clone()));
        } else if e.clears_await() {
            g.awaiting = None;
        }
        let (lvl, text) = match e {
            Event::Step(t) => (Lvl::Step, t),
            Event::Detail(t) => (Lvl::Detail, t),
            Event::Warn(t) => (Lvl::Warn, t),
            Event::Observed(t) => (Lvl::Observed, t),
            Event::Shell(t) => (Lvl::Shell, t),
            Event::Net(t) => (Lvl::Net, t),
            Event::Resolved(t) => (Lvl::Resolved, t),
            Event::Await { title, instruction } => {
                (Lvl::Step, format!("{title}\n{instruction}"))
            }
        };
        g.push(lvl, text);
    })
}

// ------------------------------------------------------------------- the app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Find,
    Check,
    Images,
    Install,
}

impl Step {
    const ALL: [Step; 4] = [Step::Find, Step::Check, Step::Images, Step::Install];

    fn title(self) -> &'static str {
        match self {
            Step::Find => "Find",
            Step::Check => "Check",
            Step::Images => "Choose",
            Step::Install => "Install",
        }
    }
}

struct App {
    shared: Arc<Mutex<Shared>>,
    step: Step,
    /// The address to install to. A discovered row fills this in, so there is
    /// one source of truth whether the user clicked or typed.
    host: String,
    /// Root password to try first — the calculated/dealer password lives here.
    /// Left blank for the common factory case, which the known logins cover.
    password: String,
    images: String,
    /// Set when the user overrides an ambiguous or wrong identification.
    forced: Option<&'static Board>,
    details: bool,
    self_install: bool,
    /// What goes on the box. Nothing until the user picks.
    with: Option<Install>,
    /// With Control4 on an EA: which Zigbee its radio runs.
    zigbee: Zigbee,
    /// The user chose an image file instead of the downloaded one.
    own_image: bool,
    /// Has the first automatic scan been kicked off?
    ///
    /// Opening the app onto a "Scan the network" button and nothing else makes
    /// the user's first act a decision they have no information for. There is
    /// exactly one thing to do on this screen, so do it.
    auto_scanned: bool,
}

impl Default for App {
    fn default() -> Self {
        App {
            shared: Arc::new(Mutex::new(Shared::default())),
            step: Step::Find,
            host: String::new(),
            password: String::new(),
            // The path `make image` writes to, so the common case is prefilled.
            images: default_images_dir(),
            forced: None,
            details: false,
            self_install: false,
            with: None,
            zigbee: Zigbee::Openhc,
            own_image: false,
            auto_scanned: false,
        }
    }
}

/// `output/images` relative to the repo, if we appear to be inside one;
/// otherwise blank so the user browses.
fn default_images_dir() -> String {
    let mut dir = std::env::current_dir().unwrap_or_default();
    for _ in 0..4 {
        let p = dir.join("output/images");
        if p.is_dir() {
            return p.display().to_string();
        }
        if !dir.pop() {
            break;
        }
    }
    String::new()
}

impl App {
    /// The board we would install for: an override if the user set one, else
    /// whatever identification found.
    fn board(&self, sh: &Shared) -> Option<&'static Board> {
        self.forced
            .or_else(|| sh.conn.as_ref().and_then(|c| c.id.board))
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}

impl App {
    /// The whole window. Split out of the `eframe::App` impl so the smoke test
    /// can lay every screen out headlessly, with no GPU and no `eframe::Frame`.
    fn render(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Take the Arc out first: `shared.lock()` borrows `shared`, not `self`,
        // so button handlers can still clone the Arc to spawn work.
        let shared = Arc::clone(&self.shared);
        let sh = &mut *shared.lock().unwrap();

        // Start scanning the moment the window opens. The first screen has one
        // action on it and no information the user could use to decide against
        // it, so making them click is a step that exists only to be taken.
        // Guarded by a flag rather than "is the list empty", which would restart
        // the scan every frame on a network with nothing on it.
        if !self.auto_scanned {
            self.auto_scanned = true;
            self.scan(sh, &shared);
        }

        // Repaint while work is in flight; otherwise egui sleeps until input and
        // the log (or a background fetch) would sit frozen.
        if sh.busy.is_some() || !sh.checked_latest {
            ctx.request_repaint_after(Duration::from_millis(150));
        }

        self.ensure_latest(sh, &shared);
        self.take_dropped_release(&ctx, sh, &shared);

        // A release image finished downloading off-thread: open it now, on the
        // UI thread, the same way a dropped file is handled.
        if let Some(path) = sh.downloaded_image.take() {
            self.images = path.display().to_string();
            self.step = Step::Images;
            self.load_release(sh, &shared);
        }

        egui::Panel::top("head").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("openHC Flasher");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.details, "Details")
                        .on_hover_text("Show raw evidence, flash offsets and the full engine log");
                    if self.details {
                        ui.label(RichText::new(format!("v{VERSION}")).weak().size(11.0));
                    }
                });
            });

            // A newer flasher on the releases page — offer the one-click update.
            let newer = sh
                .latest
                .as_ref()
                .filter(|r| r.flasher_asset().is_some())
                .filter(|r| updates::differs_from_release(VERSION, &r.tag_name))
                .map(|r| r.tag_name.clone());
            if let Some(tag) = newer {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Flasher update available: {tag}")).color(WARN));
                    if ui
                        .add_enabled(sh.busy.is_none(), egui::Button::new("Update now"))
                        .clicked()
                    {
                        self.update_flasher(sh, &shared);
                    }
                });
            }
            if let Some(msg) = sh.notice.clone() {
                ui.label(RichText::new(msg).color(OK));
            }

            // The "it is your turn" banner. Deliberately the loudest thing on
            // screen: the install genuinely cannot proceed until someone walks
            // over and holds a button, and the previous build gave no hint of
            // that at all -- it simply appeared to hang.
            if let Some((title, instruction)) = sh.awaiting.clone() {
                ui.add_space(6.0);
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(egui::Stroke::new(2.0, WARN))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new(format!("ACTION NEEDED — {title}"))
                                    .color(WARN)
                                    .strong()
                                    .size(15.0),
                            );
                        });
                        ui.add_space(4.0);
                        for line in instruction.lines() {
                            ui.label(RichText::new(line).size(13.0));
                        }
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("the log below updates as the box responds")
                                .weak()
                                .size(11.0),
                        );
                    });
                ui.add_space(4.0);
            }

            ui.add_space(2.0);
            self.stepper(ui);
            ui.add_space(6.0);
        });

        egui::Panel::bottom("foot").show(ui, |ui| {
            ui.add_space(6.0);
            self.nav(ui, sh, &shared);
            ui.add_space(6.0);
        });

        // The log is the monitor half of the tool: always up during and after an
        // install, and on demand elsewhere.
        if self.details || sh.phase.is_some() {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(170.0)
                .show(ui, |ui| self.log_panel(ui, sh));
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.step {
                Step::Find => self.step_find(ui, sh, &shared),
                Step::Check => self.step_check(ui, sh, &shared),
                Step::Images => self.step_images(ui, sh, &shared),
                Step::Install => self.step_install(ui, sh, &shared),
            });
        });
    }
}

// ------------------------------------------------------------------- chrome

impl App {
    fn stepper(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (i, s) in Step::ALL.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("›").weak());
                }
                let t = RichText::new(format!("{}. {}", i + 1, s.title()));
                ui.label(if *s == self.step { t.strong() } else { t.weak() });
            }
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let blocked = self.blocker(sh);
        ui.horizontal(|ui| {
            let back_ok = self.step != Step::Find && sh.busy.is_none();
            if ui.add_enabled(back_ok, egui::Button::new("Back")).clicked() {
                self.step = match self.step {
                    Step::Find | Step::Check => Step::Find,
                    Step::Images => Step::Check,
                    Step::Install => Step::Images,
                };
            }

            let next_ok = self.step != Step::Install && blocked.is_none() && sh.busy.is_none();
            if ui.add_enabled(next_ok, egui::Button::new("Next")).clicked() {
                match self.step {
                    Step::Find => {
                        self.step = Step::Check;
                        self.connect(sh, shared);
                    }
                    Step::Check => self.step = Step::Images,
                    Step::Images => self.step = Step::Install,
                    Step::Install => {}
                }
            }

            // Say *why* the button is dead. A greyed-out Next with no reason is
            // where a non-technical user gives up.
            if let Some(why) = blocked {
                if sh.busy.is_none() {
                    ui.label(RichText::new(why).weak());
                }
            }
            if let Some(what) = &sh.busy {
                ui.spinner();
                ui.label(RichText::new(what.clone()).weak());
            }
        });
    }

    /// Why `Next` is unavailable, or `None` when the step is satisfied.
    fn blocker(&self, sh: &Shared) -> Option<String> {
        gate(
            self.step,
            &self.host,
            sh.conn.as_ref().map(|c| &c.id),
            self.board(sh),
            self.with.is_some(),
            sh.rel_info.as_ref(),
        )
    }

    fn log_panel(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Log").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Copy").clicked() {
                    let text: String = sh
                        .log
                        .iter()
                        .map(|(_, t)| format!("{t}\n"))
                        .collect();
                    ui.ctx().copy_text(text);
                }
                if ui.button("Clear").clicked() {
                    sh.log.clear();
                }
            });
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (lvl, text) in &sh.log {
                    // Detail and net lines are the debug half — noise for the wizard.
                    if matches!(lvl, Lvl::Detail | Lvl::Net) && !self.details {
                        continue;
                    }
                    let t = RichText::new(text).monospace().size(11.0);
                    ui.label(match lvl {
                        Lvl::Step => t,
                        Lvl::Detail => t.weak(),
                        Lvl::Warn => t.color(WARN),
                        // Console evidence during a wait: present so the user
                        // can see lines arriving, small so it does not compete
                        // with the steps.
                        Lvl::Observed => t.weak().size(11.0),
                        // Commands we send: monospace and plain, so they read
                        // as "what the tool typed", distinct from the box's echo.
                        Lvl::Shell => t,
                        // Net server chatter: dim, like the other debug half.
                        Lvl::Net => t.weak().size(11.0),
                        Lvl::Resolved => t.color(OK),
                    });
                }
                if sh.log.is_empty() {
                    ui.label(RichText::new("nothing yet").weak());
                }
            });
    }

    /// A release folder or zip dropped anywhere on the window.
    fn take_dropped_release(
        &mut self,
        ctx: &egui::Context,
        sh: &mut Shared,
        shared: &Arc<Mutex<Shared>>,
    ) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.first().map(|f| f.path().to_path_buf()) {
            self.images = path.display().to_string();
            self.own_image = true;
            sh.loaded_image = None;
            self.step = Step::Images;
            self.load_release(sh, shared);
        }
    }
}

// -------------------------------------------------------------------- the steps

impl App {
    /// Scan the network and then ask each unit what it is.
    ///
    /// Split out of the button handler so the window can start one on its
    /// first frame — see `render`.
    fn scan(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        sh.found.clear();
        sh.scanned = false;
        sh.push(Lvl::Step, "scanning the local network for Control4 hardware".into());
        let password = self.password.clone();
        spawn(sh, shared, "Scanning… this takes about half a minute", move |s| {
            let found = tp::discover(true);
            {
                let mut g = s.lock().unwrap();
                g.push(
                    Lvl::Step,
                    format!("found {} unit(s); asking each what it is", found.len()),
                );
                g.found = found
                    .iter()
                    .map(|f| Unit {
                        ip: f.ip.clone(),
                        mac: f.mac.clone(),
                        sddp: f.via.iter().any(|v| v == "sddp"),
                        sddp_name: f.name.clone(),
                        sddp_model: f.model.clone(),
                        version: None,
                        hostname: None,
                        running: None,
                        board: None,
                        probed: false,
                        note: None,
                    })
                    .collect();
                g.scanned = true;
            }
            // One probe per unit, in parallel. A unit that refuses every
            // login burns the full SSH timeout, and one slow box must not
            // hold up the rest of the list.
            let probes: Vec<_> = found
                .into_iter()
                .map(|f| {
                    let s = Arc::clone(s);
                    let pw = extra_passwords(&password);
                    std::thread::spawn(move || probe_unit(&s, &f.ip, &pw))
                })
                .collect();
            for h in probes {
                let _ = h.join();
            }
        });
    }

    fn step_find(&mut self, ui: &mut egui::Ui, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        ui.heading("Find your controller");
        ui.label("Plug the controller into the same network as this computer and turn it on.");
        ui.add_space(10.0);

        if ui
            .add_enabled(sh.busy.is_none(), egui::Button::new("Scan again"))
            .clicked()
        {
            self.scan(sh, shared);
        }

        ui.add_space(8.0);
        if sh.found.is_empty() {
            if sh.scanned && sh.busy.is_none() {
                ui.label(
                    RichText::new(
                        "No controllers found. A freshly restored unit can take a minute to \
                         appear — scan again, or type its address below.",
                    )
                    .color(WARN),
                );
            }
        } else {
            ui.label(RichText::new("Click the controller to install:").strong());
            ui.add_space(4.0);
            for u in &sh.found {
                let mut label = format!("{:<16}{}", u.ip, u.display_name());
                label.push('\n');
                label.push_str(&u.firmware());
                if let Some(b) = u.board {
                    label.push_str(&format!("  ·  {}", b.desc));
                }
                if self.details {
                    label.push_str(&format!("\n{}", u.mac.clone().unwrap_or_default()));
                }
                if ui
                    .selectable_label(self.host == u.ip, RichText::new(label).monospace().size(12.0))
                    .clicked()
                {
                    self.host = u.ip.clone();
                }
                ui.add_space(2.0);
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label("Address:");
            ui.add(
                egui::TextEdit::singleline(&mut self.host)
                    .hint_text("192.168.1.50")
                    .desired_width(200.0),
            );
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Password:");
            ui.add(
                egui::TextEdit::singleline(&mut self.password)
                    .password(true)
                    .hint_text("leave blank for a factory controller")
                    .desired_width(240.0),
            );
        });
        ui.label(
            RichText::new(
                "Only needed if a dealer changed the controller's root password, or a factory \
                 unit no longer accepts the default one.",
            )
            .weak()
            .size(11.0),
        );
    }

    fn step_check(&mut self, ui: &mut egui::Ui, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        ui.heading("What is this controller?");
        ui.add_space(8.0);

        if let Some(err) = &sh.connect_error {
            ui.label(RichText::new(err.clone()).color(BAD));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Password:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(240.0),
                );
                if ui
                    .add_enabled(sh.busy.is_none(), egui::Button::new("Try again"))
                    .clicked()
                {
                    self.connect(sh, shared);
                }
            });
            return;
        }
        let Some(conn) = &sh.conn else {
            if sh.busy.is_none() {
                if ui.button("Connect").clicked() {
                    self.connect(sh, shared);
                }
            }
            return;
        };

        let board = self.board(sh);
        let installed = conn.installed_version.clone();
        let latest_tag = sh.latest.as_ref().map(|r| r.tag_name.clone());
        egui::Grid::new("id")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Address").weak());
                ui.monospace(&conn.host);
                ui.end_row();

                ui.label(RichText::new("Model").weak());
                match board {
                    Some(b) => {
                        ui.label(RichText::new(b.desc).strong());
                    }
                    None => {
                        ui.label(RichText::new("could not be identified").color(BAD));
                    }
                }
                ui.end_row();

                ui.label(RichText::new("Running").weak());
                ui.label(running_text(conn.id.running));
                ui.end_row();

                // The Control4 OS version, for a unit that is still on stock.
                // It decides whether a default root password exists at all
                // (removed in 3.1.0), so it is the single most useful fact on
                // this screen for anyone who cannot log in.
                if conn.id.running != Running::Openhc {
                    if let Some(v) = &conn.id.version {
                        ui.label(RichText::new("Control4 OS").weak());
                        ui.monospace(v);
                        ui.end_row();
                    }
                }
                if let Some(v) = &installed {
                    ui.label(RichText::new("openHC version").weak());
                    ui.monospace(v);
                    ui.end_row();
                }
                if let Some(t) = &latest_tag {
                    ui.label(RichText::new("Latest release").weak());
                    ui.monospace(t);
                    ui.end_row();
                }
            });

        // Tell the user, in plain terms, whether this box is current.
        if let (Some(inst), Some(tag)) = (&installed, &latest_tag) {
            ui.add_space(6.0);
            if updates::differs_from_release(inst, tag) {
                ui.label(
                    RichText::new(format!(
                        "This controller is not on the latest openHC release ({tag}). \
                         Installing below updates it."
                    ))
                    .color(WARN),
                );
            } else {
                ui.label(RichText::new("Running the latest openHC release.").color(OK));
            }
        }

        // A board the table doesn't know: offer a prefilled issue, never send one.
        if let Some(u) = &conn.id.unknown {
            let model = sh
                .found
                .iter()
                .find(|f| f.ip == conn.host)
                .and_then(|f| f.sddp_model.clone())
                .or_else(|| conn.id.raw.iter().find(|(k, _)| k == "name").map(|(_, v)| v.trim().to_string()));
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "Unknown board: {}. The flasher's table doesn't have it yet.",
                    u.describe()
                ))
                .color(WARN),
            );
            ui.hyperlink_to(
                "Report it on GitHub (opens a prefilled issue; nothing is sent until you submit it)",
                board::unknown_board_issue_url(u, model.as_deref(), VERSION),
            );
        }

        // An uncertain identification is the one thing that must never be
        // papered over: the wrong image on the wrong board costs a recovery.
        if !conn.id.certain() && self.forced.is_none() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "This model could not be pinned down from the controller alone. \
                     Choose it below — check the label on the underside of the unit.",
                )
                .color(WARN),
            );
        }

        ui.add_space(10.0);
        let choices: Vec<&'static Board> = if self.details || !conn.id.certain() {
            if conn.id.candidates.len() > 1 {
                conn.id.candidates.clone()
            } else {
                board::BOARDS.iter().collect()
            }
        } else {
            vec![]
        };
        if !choices.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Model:");
                let current = board.map(|b| b.name).unwrap_or("choose…");
                egui::ComboBox::from_id_salt("board")
                    .selected_text(current)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for b in choices {
                            let sel = board.is_some_and(|c| c.name == b.name);
                            if ui.selectable_label(sel, format!("{}  —  {}", b.name, b.desc)).clicked() {
                                self.forced = Some(b);
                            }
                        }
                    });
                if self.forced.is_some() && ui.button("Reset").clicked() {
                    self.forced = None;
                }
            });
        }

        let Some(b) = board else { return };

        // Identity for method choice: an override replaces what was detected,
        // but the *running* state still comes from the box.
        let id = Identity {
            board: Some(b),
            candidates: vec![b],
            running: conn.id.running,
            version: None,
            raw: conn.id.raw.clone(),
            warning: None,
            unknown: None,
        };
        let (chosen, rejected) = method::choose(&id, None);

        ui.add_space(12.0);
        match chosen {
            Some(m @ (Method::Network | Method::Kexec | Method::Grub | Method::Nand | Method::Sd)) => {
                let plan = method::plan(b, m);
                // Said before anything else, because on the HC-800 the default
                // method writes to no partition at all, and that is the single
                // most reassuring thing anyone reading this screen can be told.
                if m.writes_nothing() {
                    ui.label(RichText::new("This writes nothing to the controller.").strong());
                    ui.add_space(6.0);
                }
                ui.label(RichText::new("What the install will do").strong());
                ui.add_space(4.0);
                for s in &plan.steps {
                    ui.label(format!("• {s}"));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("If you change your mind").strong());
                ui.label(&plan.reversible);

                if self.details {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(RichText::new("Details").strong());
                    ui.label(RichText::new("evidence").weak());
                    for (k, v) in &conn.id.raw {
                        ui.monospace(format!("  {k} = {}", v.trim()));
                    }
                    ui.label(RichText::new("writes").weak());
                    for w in &plan.writes {
                        ui.monospace(format!("  {w}"));
                    }
                    for (m, why) in &rejected {
                        ui.label(RichText::new(format!("not {}: {why}", m.name())).weak());
                    }
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.self_install, "One-shot self-install (advanced)")
                        .on_hover_text(
                            "Reboots once and lets a tiny on-device installer write p1, instead \
                             of the two-stage flow. Faster, but a fault leaves a controller that \
                             is only recoverable over a serial console.",
                        );
                }
            }
            Some(other) => {
                ui.label(
                    RichText::new(format!(
                        "This controller installs with the {} method, which this app cannot \
                         drive yet. Use the ohc-flash command line tool.",
                        other.name()
                    ))
                    .color(BAD),
                );
            }
            None => {
                ui.label(RichText::new("No install method applies to this controller.").color(BAD));
                for (m, why) in &rejected {
                    ui.label(RichText::new(format!("not {}: {why}", m.name())).weak());
                }
            }
        }
    }

    fn step_images(&mut self, ui: &mut egui::Ui, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let Some(b) = self.board(sh) else {
            ui.label("Go back and identify the controller first.");
            return;
        };
        ui.heading("What should this controller run?");
        ui.label("The flasher downloads what it needs.");
        ui.add_space(10.0);

        for i in Install::ALL.into_iter().filter(|i| i.fits(b)) {
            let rel = if i.image().0 == choice::HA_REPO { &sh.img_ha } else { &sh.img_openhc };
            // Unknown until GitHub answers; only a release without this board's
            // image rules a choice out.
            let missing = rel.as_ref().is_some_and(|r| r.image_asset_named(i.image().1, b.name).is_none());
            let usable = self.own_image || !missing;
            let r = ui.add_enabled(
                usable && sh.busy.is_none(),
                egui::RadioButton::new(self.with == Some(i), RichText::new(i.title()).strong()),
            );
            if r.clicked() && self.with != Some(i) {
                self.with = Some(i);
                if !self.own_image && sh.loaded_image != Some(i.image()) {
                    self.download_image(sh, shared, b, i);
                }
            }
            ui.indent(i.name(), |ui| {
                ui.label(RichText::new(i.blurb()).weak());
                if missing && !self.own_image {
                    ui.label(RichText::new("No image for this controller has been published yet.").weak());
                }
                if i == Install::Control4 && self.with == Some(i) && Zigbee::choosable(b) {
                    ui.add_space(4.0);
                    ui.label("Its Zigbee radio:");
                    ui.radio_value(&mut self.zigbee, Zigbee::Openhc, "openHC Zigbee");
                    ui.indent("zb-openhc", |ui| {
                        ui.label(
                            RichText::new(
                                "Standard Zigbee 3.0 devices (bulbs, sensors, switches from any brand), \
                                 on the box's MQTT.",
                            )
                            .weak(),
                        );
                    });
                    ui.radio_value(&mut self.zigbee, Zigbee::Control4, "Control4 Zigbee (experimental)");
                    ui.indent("zb-c4", |ui| {
                        ui.label(
                            RichText::new(
                                "The radio becomes a Control4 Zigbee access point, for Control4's own \
                                 Zigbee devices. Not working yet: Director does not connect to it.",
                            )
                            .color(WARN),
                        );
                    });
                }
            });
            ui.add_space(6.0);
        }
        if sh.img_openhc.is_none() && sh.img_ha.is_none() {
            if let Some(e) = &sh.latest_error {
                ui.label(RichText::new(format!("GitHub did not answer: {e}")).color(WARN));
            }
        }

        ui.add_space(6.0);
        egui::CollapsingHeader::new("Use my own image (a folder or .zip)")
            .default_open(self.own_image)
            .show(ui, |ui| {
                ui.label(RichText::new("A build of your own, or a zip you downloaded. You can also drag one onto this window.").weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let edited = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.images)
                                .hint_text("folder or .zip")
                                .desired_width(380.0),
                        )
                        .lost_focus();
                    if ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("openHC release", &["zip"])
                            .pick_file()
                            .or_else(|| rfd::FileDialog::new().pick_folder())
                        {
                            self.images = p.display().to_string();
                            self.own_image = true;
                            self.load_release(sh, shared);
                        }
                    }
                    if ui.button("Folder…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.images = p.display().to_string();
                            self.own_image = true;
                            self.load_release(sh, shared);
                        }
                    }
                    if edited && !self.images.trim().is_empty() {
                        self.own_image = true;
                        self.load_release(sh, shared);
                    }
                });
            });

        ui.add_space(12.0);
        if let Some(err) = &sh.rel_error {
            ui.label(RichText::new(format!("✗ {err}")).color(BAD));
            return;
        }
        let Some(info) = &sh.rel_info else {
            if sh.busy.is_none() && !self.images.trim().is_empty() {
                if ui.button("Check this release").clicked() {
                    self.load_release(sh, shared);
                }
            }
            return;
        };

        if info.problems.is_empty() {
            // The "spare" figure is the room left under CEFDK's copy window, and
            // that window only exists on the EA family. The HC-800 boots from
            // GRUB with no such limit, so quoting a headroom there is noise at
            // best and a scary negative number at worst.
            let msg = if info.hc800 {
                format!("✓ Ready — kernel {}, initramfs present", fmt_bytes(info.kernel_len))
            } else {
                let head = image::headroom(info.kernel_len);
                format!(
                    "✓ Ready — kernel {} ({} spare), root filesystem present",
                    fmt_bytes(info.kernel_len),
                    fmt_bytes(head.max(0) as u64),
                )
            };
            ui.label(RichText::new(msg).color(OK));
        } else {
            for p in &info.problems {
                ui.label(RichText::new(format!("✗ {p}")).color(BAD));
            }
        }

        // The advanced one-shot path needs two extra artefacts; say so here
        // rather than failing halfway through an install.
        if self.self_install {
            for need in ["boot-init.cpio.gz", "rootfs.ext2.gz"] {
                if !info.names.iter().any(|n| n == need) {
                    ui.label(
                        RichText::new(format!("✗ self-install needs {need}, not in this release"))
                            .color(BAD),
                    );
                }
            }
        }

        if self.details {
            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Details").strong());
            ui.monospace(&info.source);
            for n in &info.names {
                ui.monospace(format!("  {n}"));
            }
        }
    }

    fn step_install(&mut self, ui: &mut egui::Ui, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let Some(b) = self.board(sh) else {
            ui.label("Go back and identify the controller first.");
            return;
        };

        match sh.phase {
            None => {
                ui.heading("Ready to install");
                ui.add_space(8.0);
                egui::Grid::new("summary")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Controller").weak());
                        ui.label(RichText::new(b.desc).strong());
                        ui.end_row();
                        ui.label(RichText::new("Address").weak());
                        ui.monospace(&self.host);
                        ui.end_row();
                        ui.label(RichText::new("Installing").weak());
                        let with = self.with.map(Install::title).unwrap_or("(nothing chosen)");
                        let zb = match self.zigbee {
                            Zigbee::Control4 if self.with == Some(Install::Control4) && Zigbee::choosable(b) => {
                                ", Control4 Zigbee"
                            }
                            _ => "",
                        };
                        ui.label(RichText::new(format!("{with}{zb}")).strong());
                        ui.end_row();
                        ui.label(RichText::new("Image").weak());
                        ui.monospace(&self.images);
                        ui.end_row();
                    });
                ui.add_space(12.0);
                ui.label(
                    "This takes a few minutes and the controller restarts on its own. \
                     Leave it powered and on the network until this window says it is done.",
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        method_for(b).map(|m| method::plan(b, m).reversible).unwrap_or_default(),
                    )
                    .weak(),
                );
                ui.add_space(14.0);
                if ui
                    .add_enabled(
                        sh.busy.is_none(),
                        egui::Button::new(RichText::new("Install openHC").strong()),
                    )
                    .clicked()
                {
                    self.start_install(sh, shared, false);
                }
            }
            Some(Phase::Done) if sh.control4_driver.is_some() => {
                let driver = sh.control4_driver.clone().unwrap_or_default();
                let on_box = control4::runs_on_box(b);
                ui.heading(RichText::new("Done").color(OK));
                ui.add_space(8.0);
                ui.label("openHC is installed and it is a Control4 controller again. In Composer Pro:");
                ui.add_space(6.0);
                let add = format!("Driver → Add Driver… and pick {}.", driver.display());
                let steps: [String; 3] = if on_box {
                    [
                        add,
                        "Add the openHC box from the discovered devices. It brings its own address.".into(),
                        "Bind HDMI to the TV's input, and the relays, contacts, IR and serial it has.".into(),
                    ]
                } else {
                    [
                        add,
                        "On the Network tab, identify its openHC Controller connection with the box's \
                         address (Find it again here if a reboot changed it)."
                            .into(),
                        "Bind the relays, contacts, IR and serial it has.".into(),
                    ]
                };
                for (n, step) in steps
                .iter()
                .enumerate()
                {
                    ui.label(format!("{}. {step}", n + 1));
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(if on_box {
                        "The room's C4 button then opens the navigator on that TV. The driver's \
                         Documentation tab shows the box's own dashboard."
                    } else {
                        "The driver's Documentation tab shows the box's own dashboard."
                    })
                    .weak(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Show the driver").clicked() {
                        reveal(driver.parent().unwrap_or(&driver));
                    }
                    if ui.add_enabled(sh.busy.is_none(), egui::Button::new("Install another")).clicked() {
                        self.reset(sh);
                    }
                });
            }
            Some(Phase::Done) if matches!(self.with, Some(Install::HomeAssistant | Install::MusicAssistant)) => {
                ui.heading(RichText::new("Done").color(OK));
                ui.add_space(8.0);
                ui.label(
                    "openHC is installed and the controller is restarting. On this first start it \
                     downloads and installs its apps, which takes a while on this hardware.",
                );
                ui.add_space(6.0);
                let ha = self.with == Some(Install::HomeAssistant);
                let mut steps = vec![format!(
                    "openHC's own page is up in about a minute: http://{}/ (its address may change \
                     on reboot; Find it again below).",
                    self.host
                )];
                if ha {
                    steps.push(format!(
                        "Home Assistant: http://{}:8123 in 20–40 minutes. Create your account, then add \
                         the MQTT integration (broker 127.0.0.1) to get the box's IO, Zigbee and Bluetooth.",
                        self.host
                    ));
                }
                steps.push(format!(
                    "Music Assistant: http://{}:8095. Create its admin account within 15 minutes of it \
                     starting; the box's outputs are its squeezelite players.",
                    self.host
                ));
                for (n, step) in steps.iter().enumerate() {
                    ui.label(format!("{}. {step}", n + 1));
                }
                ui.add_space(4.0);
                ui.hyperlink_to(
                    "The full guide",
                    format!("https://github.com/{}/blob/main/docs/install.md", choice::HA_REPO),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(sh.busy.is_none(), egui::Button::new("Find it again")).clicked() {
                        self.reset(sh);
                    }
                });
            }
            Some(Phase::Done) => {
                ui.heading(RichText::new("Done").color(OK));
                ui.add_space(8.0);
                if let Some(e) = &sh.control4_error {
                    ui.label(RichText::new(format!("openHC is in, but the Control4 setup did not finish: {e}")).color(WARN));
                    ui.add_space(6.0);
                }
                ui.label(
                    "openHC is installed and the controller is restarting. Give it about a \
                     minute to come up.",
                );
                ui.add_space(6.0);
                // The single most useful thing to say here, learned the hard
                // way: the controller takes a NEW DHCP lease when it reboots, so
                // its address usually changes. Telling someone to reopen the old
                // one is how a good install looks like a failure. Point them at
                // finding it afresh and at the on-device dashboard.
                ui.label(
                    RichText::new(
                        "Its address may change on reboot — find it again below rather than \
                         reusing the old one. Once it is up, open its web dashboard in a browser \
                         to manage it.",
                    )
                    .weak(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(sh.busy.is_none(), egui::Button::new("Find it again"))
                        .clicked()
                    {
                        // Straight back to a fresh scan — the loop a user
                        // actually wants after an install, not a blank Find
                        // screen they have to re-drive.
                        self.reset(sh);
                    }
                    if ui.button("Install another").clicked() {
                        self.reset(sh);
                    }
                });
            }
            Some(Phase::Failed) => {
                ui.heading(RichText::new("Install failed").color(BAD));
                ui.add_space(8.0);
                if let Some(e) = &sh.error {
                    ui.label(RichText::new(e.clone()).color(BAD));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    // Stage 1 already landed, so the controller is sitting in
                    // RAM waiting for its root filesystem. Offer exactly that
                    // rather than restarting a flow that would redo the kernel.
                    if sh.stage1_done
                        && ui
                            .add_enabled(sh.busy.is_none(), egui::Button::new("Finish install"))
                            .clicked()
                    {
                        self.start_install(sh, shared, true);
                    }
                    if ui
                        .add_enabled(sh.busy.is_none(), egui::Button::new("Start over"))
                        .clicked()
                    {
                        self.reset(sh);
                    }
                });
            }
            Some(phase) => {
                ui.heading("Installing");
                ui.add_space(10.0);
                let (label, frac) = match phase {
                    Phase::Writing => ("Writing openHC to the controller", None),
                    Phase::Restarting => (
                        "The controller is restarting",
                        sh.wait.map(|(start, end)| {
                            let total = end.duration_since(start).as_secs_f32().max(1.0);
                            (start.elapsed().as_secs_f32() / total).clamp(0.0, 1.0)
                        }),
                    ),
                    Phase::Finishing => ("Writing the system files", None),
                    Phase::Control4 => ("Setting it up for Control4", None),
                    _ => unreachable!(),
                };
                ui.label(RichText::new(label).strong());
                ui.add_space(6.0);
                match frac {
                    Some(f) => {
                        ui.add(egui::ProgressBar::new(f).show_percentage());
                    }
                    None => {
                        ui.add(egui::ProgressBar::new(0.0).animate(true).text("working"));
                    }
                }
                ui.add_space(8.0);
                if let Some((_, text)) = sh.log.iter().rev().find(|(l, _)| *l == Lvl::Step) {
                    ui.label(RichText::new(text).weak());
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Do not unplug the controller.").color(WARN));
            }
        }
    }
}

// ------------------------------------------------------------------- the jobs

impl App {
    fn connect(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let host = self.host.trim().to_string();
        let passwords = extra_passwords(&self.password);
        self.forced = None;
        sh.conn = None;
        sh.connect_error = None;
        sh.push(Lvl::Step, format!("connecting to {host}"));
        spawn(sh, shared, "Connecting…", move |s| {
            let Some(ssh) = tp::first_working_login_with(&host, &passwords) else {
                // Reachable-but-rejected is a different problem, and a different
                // fix, from a box that is not answering at all.
                // NAME THE LIKELY CAUSE. The old text blamed "a dealer-updated
                // controller" with "a calculated root password", which is rare
                // and unactionable. The common case is neither: Control4
                // REMOVED the default root password in OS 3.1.0, so on any
                // reasonably current controller there is nothing to guess and
                // no amount of retrying will help. Measured both ways — the
                // factory password still works on an HC-800 on the 2.x image,
                // and an EA-3 on 3.3.3 refuses it.
                let msg = if tp::ssh_port_open(&host) {
                    format!(
                        "{host} answered, but refused every login.\n\n\
                         Control4 OS 3.1.0 and later have no default root password — it was \
                         removed, so there is nothing to guess. Add an SSH user with Composer's \
                         Network Tools and enter it below, or use an SSH key with the ohc-flash \
                         command line.\n\n\
                         On OS 3.0.x and earlier the factory login is root / t0talc0ntr0l4!, \
                         which this already tried."
                    )
                } else {
                    format!(
                        "Could not reach {host} over SSH. Check the address, and that the \
                         controller is powered on and on this network."
                    )
                };
                let mut g = s.lock().unwrap();
                g.push(Lvl::Warn, format!("no working login for {host}"));
                g.connect_error = Some(msg);
                return;
            };
            let id = tp::identify(&ssh);
            let installed_version = read_installed_version(&ssh);
            let mut g = s.lock().unwrap();
            g.push(Lvl::Step, format!("{host}: {}", id.describe()));
            if let Some(v) = &installed_version {
                g.push(Lvl::Detail, format!("openHC version = {v}"));
            }
            for (k, v) in &id.raw {
                g.push(Lvl::Detail, format!("{k} = {}", v.trim()));
            }
            if let Some(u) = &id.unknown {
                g.push(Lvl::Warn, format!("unknown board: {} is not in the flasher's table yet", u.describe()));
            }
            g.conn = Some(Conn { host, ssh, id, installed_version });
        });
    }

    fn load_release(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let path = PathBuf::from(self.images.trim());
        // Which board this release is FOR decides how it is judged: an HC-800
        // release legitimately has no rootfs.ext2 and a kernel far past the EA
        // size window, so validating it by the EA rules reports two problems
        // that are not problems and blocks a good release. Same split the CLI's
        // `validate` makes; kept here rather than in one shared place only
        // because the GUI reads the board off live connection state.
        let family = self.board(sh).map(|b| b.family);
        sh.rel = None;
        sh.rel_info = None;
        sh.rel_error = None;
        sh.push(Lvl::Step, format!("reading release {}", path.display()));
        spawn(sh, shared, "Reading the release…", move |s| {
            match Release::open(Path::new(&path)) {
                Err(e) => {
                    let mut g = s.lock().unwrap();
                    g.push(Lvl::Warn, format!("{e:#}"));
                    g.rel_error = Some(format!("{e:#}"));
                }
                Ok(rel) => {
                    // HC-800 bundles carry the kernel under a prefixed name and
                    // no bzImage; fall back to it so the size and magic checks
                    // see the real kernel rather than nothing.
                    let kern = rel
                        .get("openhc-hc800-kernel.img")
                        .or_else(|| rel.get("bzImage"));
                    let head = kern.map(|b| &b[..b.len().min(0x400)]);
                    let klen = kern.map(|b| b.len() as u64).unwrap_or(0);
                    let hc = matches!(family, Some(Family::Hc));
                    let problems = if matches!(family, Some(Family::Omap)) {
                        let mut v: Vec<String> = match rel.get(ohc_flash_core::hc250::IMAGE_FILE) {
                            Some(img) => ohc_flash_core::hc250::check_image(img).err().into_iter().collect(),
                            None => vec![format!("no {}", ohc_flash_core::hc250::IMAGE_FILE)],
                        };
                        for k in ["ohc_kexec.ko", "ohc-kexec.sh", "ohc-boot-env.sh"] {
                            if !rel.has(k) {
                                v.push(format!("no {k} (the HC-250's stock-side kit)"));
                            }
                        }
                        v
                    } else if hc {
                        let initrd = rel.has("rootfs.cpio.gz") || rel.has("openhc-initrd.gz");
                        image::hc_problems(head, klen, initrd)
                    } else {
                        image::ea_problems(head, klen, rel.has("rootfs.ext2"), true)
                    };
                    let mut names: Vec<String> =
                        rel.names().into_iter().map(str::to_string).collect();
                    names.sort();
                    let info = RelInfo {
                        source: rel.source.clone(),
                        kernel_len: klen,
                        hc800: hc,
                        problems,
                        names,
                    };
                    let mut g = s.lock().unwrap();
                    for p in &info.problems {
                        g.push(Lvl::Warn, p.clone());
                    }
                    g.push(
                        Lvl::Detail,
                        format!("release holds: {}", info.names.join(", ")),
                    );
                    g.rel_info = Some(info);
                    g.rel = Some(Arc::new(rel));
                }
            }
        });
    }

    /// Run the install. `finish_only` resumes a run whose stage 1 already
    /// landed — the controller is in RAM and only needs its root filesystem.
    fn start_install(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>, finish_only: bool) {
        let (Some(b), Some(rel)) = (self.board(sh), sh.rel.clone()) else { return };
        let Some(conn) = &sh.conn else { return };
        let (ssh, host) = (conn.ssh.clone(), conn.host.clone());
        let secure = b.secure_boot;
        let self_install = self.self_install;
        let Some(m) = method_for(b) else { return };
        let with = self.with.unwrap_or(Install::Openhc);
        let c4 = with == Install::Control4 && !finish_only;
        let zigbee = if Zigbee::choosable(b) { self.zigbee } else { Zigbee::Openhc };
        let on_box = control4::runs_on_box(b);
        // The MAC finds the box again after its reboot, on whatever lease it gets.
        let mac = sh.found.iter().find(|u| u.ip == host).and_then(|u| u.mac.clone());

        sh.error = None;
        sh.control4_driver = None;
        sh.control4_error = None;
        sh.phase = Some(if finish_only { Phase::Restarting } else { Phase::Writing });
        if !finish_only {
            sh.stage1_done = false;
        }

        spawn(sh, shared, "Installing…", move |s| {
            let p = sink(s);
            let files = with.p1_files();
            let out = run_install(s, &p, &ssh, &host, &rel, m, secure, self_install, finish_only, &files);
            if out.is_ok() && c4 {
                s.lock().unwrap().phase = Some(Phase::Control4);
                let r = run_control4(&p, &host, mac.as_deref(), b, on_box, zigbee);
                let mut g = s.lock().unwrap();
                match r {
                    Ok(driver) => g.control4_driver = Some(driver),
                    Err(e) => {
                        g.push(Lvl::Warn, e.clone());
                        g.control4_error = Some(e);
                    }
                }
            }
            let mut g = s.lock().unwrap();
            g.wait = None;
            match out {
                Ok(()) => g.phase = Some(Phase::Done),
                Err(e) => {
                    g.push(Lvl::Warn, e.clone());
                    g.error = Some(e);
                    g.phase = Some(Phase::Failed);
                }
            }
        });
    }

    /// Fetch the latest release once, in the background, without blocking the
    /// UI (no `busy`): it only fills in the update hints and the download
    /// button, so the wizard stays usable whether or not GitHub answers.
    fn ensure_latest(&self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        if sh.checked_latest {
            return;
        }
        sh.checked_latest = true;
        let s = Arc::clone(shared);
        std::thread::spawn(move || {
            match updates::latest_release() {
                Ok(r) => s.lock().unwrap().latest = Some(r),
                Err(e) => s.lock().unwrap().latest_error = Some(format!("{e:#}")),
            }
            let openhc = updates::newest_from(updates::REPO).ok();
            let ha = updates::newest_from(choice::HA_REPO).ok();
            let mut g = s.lock().unwrap();
            (g.img_openhc, g.img_ha) = (openhc, ha);
        });
    }

    /// Download the newest image for what the user chose and open it.
    fn download_image(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>, b: &'static Board, i: Install) {
        sh.rel = None;
        sh.rel_info = None;
        sh.rel_error = None;
        sh.loaded_image = None;
        spawn(sh, shared, "Downloading…", move |s| {
            let p = sink(s);
            match i.fetch_image(b, &p) {
                Ok(dest) => {
                    let mut g = s.lock().unwrap();
                    g.downloaded_image = Some(dest);
                    g.loaded_image = Some(i.image());
                }
                Err(e) => {
                    let mut g = s.lock().unwrap();
                    g.push(Lvl::Warn, format!("{e:#}"));
                    g.rel_error = Some(format!("{e:#}"));
                }
            }
        });
    }

    /// Download the newest flasher build and swap this binary for it.
    fn update_flasher(&mut self, sh: &mut Shared, shared: &Arc<Mutex<Shared>>) {
        let Some(rel) = &sh.latest else { return };
        let Some(asset) = rel.flasher_asset() else { return };
        let (url, name) = (asset.url.clone(), asset.name.clone());
        sh.notice = None;
        sh.push(Lvl::Step, format!("downloading {name}"));
        spawn(sh, shared, "Updating the flasher…", move |s| {
            let exe = match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => {
                    s.lock().unwrap().notice = Some(format!("cannot find this program on disk: {e}"));
                    return;
                }
            };
            // Download beside the current binary so the final step is an atomic
            // same-directory rename.
            let dest = exe.with_file_name(&name);
            let msg = match updates::download(&url, &dest) {
                Ok(()) => match updates::resolve_executable(&dest) {
                    Ok(exe_file) => apply_flasher_update(&exe_file).unwrap_or_else(|e| e),
                    Err(e) => format!("{e:#}"),
                },
                Err(e) => format!("{e:#}"),
            };
            let _ = std::fs::remove_file(&dest); // clean the download (a macOS .app zip leaves this behind)
            let mut g = s.lock().unwrap();
            g.push(Lvl::Step, msg.clone());
            g.notice = Some(msg);
        });
    }

    fn reset(&mut self, sh: &mut Shared) {
        sh.phase = None;
        sh.error = None;
        sh.stage1_done = false;
        sh.conn = None;
        sh.found.clear();
        sh.scanned = false;
        sh.log.clear();
        self.forced = None;
        self.step = Step::Find;
        // Returning to Find always means a fresh look — a different unit for
        // "Install another", a new DHCP lease for "Find it again". Re-arm the
        // launch scan so the list is never stale on arrival.
        self.auto_scanned = false;
    }
}

/// Read the openHC version a box reports. `/etc/openhc-release` carries a
/// `version=` line (stamped by board/common/post-build.sh); `board.env`'s
/// `OHC_VERSION` is the fallback for an older overlay that predates it.
fn read_installed_version(ssh: &tp::Ssh) -> Option<String> {
    let from = |text: String, key: &str| -> Option<String> {
        text.lines().find_map(|l| {
            l.trim()
                .strip_prefix(key)?
                .trim()
                .trim_matches('"')
                .to_string()
                .into()
        })
    };
    ssh.read_file("/etc/openhc-release")
        .and_then(|t| from(t, "version="))
        .or_else(|| ssh.read_file("/opt/ohc/board.env").and_then(|t| from(t, "OHC_VERSION=")))
        .filter(|v| !v.is_empty())
}

/// Replace this running flasher with a freshly downloaded build.
///
/// `self_replace` does the platform-correct thing: an atomic rename-over on
/// Unix, and on Windows the rename-current-aside / move-new-in dance that gets
/// around a running .exe being unoverwritable (Windows forbids overwriting a
/// running image but allows *renaming* it). Either way the path ends up holding
/// the new binary; the change takes effect on the next launch.
fn apply_flasher_update(downloaded: &std::path::Path) -> Result<String, String> {
    self_replace::self_replace(downloaded)
        .map_err(|e| format!("could not replace this program: {e}"))?;
    let _ = std::fs::remove_file(downloaded); // the copy is in place; source is spare
    Ok("Updated. Quit and reopen openHC Flasher to run the new version.".into())
}

/// Log into one discovered unit and record its hostname and firmware.
///
/// The hostname comes from the box rather than from reverse DNS: it is the name
/// the unit actually answers to, and openHC sets a recognisable one
/// (`openhc-<board>-<mac>`), so the list distinguishes a converted controller
/// from a stock one at a glance.
fn probe_unit(s: &Arc<Mutex<Shared>>, ip: &str, passwords: &[String]) {
    let login = tp::first_working_login_with(ip, passwords);
    let hostname = login
        .as_ref()
        .and_then(|ssh| ssh.read_file("/proc/sys/kernel/hostname"))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    let id = login.as_ref().map(tp::identify);

    let mut g = s.lock().unwrap();
    if let Some(u) = g.found.iter_mut().find(|u| u.ip == ip) {
        u.probed = true;
        u.hostname = hostname;
        match &id {
            Some(i) => {
                u.running = Some(i.running);
                u.board = i.board;
                u.version = i.version.clone();
            }
            // Not a failure worth alarming anyone about: a stock unit with
            // changed credentials still installs once the user supplies them.
            None if u.sddp => u.note = Some("needs a password".into()),
            None => u.note = Some("could not log in".into()),
        }
    }
    let line = match &id {
        Some(i) => format!("{ip}: {}", i.describe()),
        None => format!("{ip}: no working login"),
    };
    g.push(Lvl::Detail, line);
}

/// Which method this board installs with.
///
/// Reaching the install screen means we are logged into a running system, so
/// `Stock` stands in for the live identity — every method that could be chosen
/// here requires one anyway, and the board is what actually decides.
fn method_for(b: &'static ohc_flash_core::board::Board) -> Option<Method> {
    let id = Identity {
        board: Some(b),
        candidates: vec![b],
        running: ohc_flash_core::board::Running::Stock,
            version: None,
        raw: vec![],
        warning: None,
        unknown: None,
    };
    method::choose(&id, None).0
}

/// The install sequence itself, off the UI thread. Mirrors the CLI's `install`
/// so both front ends drive the engine identically.
#[allow(clippy::too_many_arguments)]
fn run_install(
    s: &Arc<Mutex<Shared>>,
    p: &Progress,
    ssh: &tp::Ssh,
    host: &str,
    rel: &Release,
    method: Method,
    secure_boot: bool,
    self_install: bool,
    finish_only: bool,
    files: &[(&str, &[u8])],
) -> Result<(), String> {
    // The HC-800 flows are SINGLE STAGE. The EA install has to reboot into a RAM
    // installer halfway through because p1 is the running root and a mounted
    // root cannot be overwritten; neither HC-800 method has that problem — kexec
    // writes nothing at all, and the grub install only touches partitions that
    // are not mounted. So they finish here rather than falling into the
    // wait-for-reboot machinery below, which would sit watching an address that
    // was never going to change.
    match method {
        // The HC-250's kexec is its own (a module, no kexec-tools); the release
        // says which board it is for.
        Method::Kexec if rel.has(ohc_flash_core::hc250::IMAGE_FILE) => {
            return hc250::kexec(ssh, rel, p).map_err(|e| format!("{e:#}"));
        }
        Method::Kexec => {
            return hc800::kexec(ssh, rel, None, p).map_err(|e| format!("{e:#}"));
        }
        // Waits for its own reboots (stock -> RAM -> stock -> SD) internally.
        Method::Sd => {
            return hc250::install_sd(ssh, rel, p).map_err(|e| format!("{e:#}"));
        }
        Method::Grub => {
            // The GUI installs openHC as the DEFAULT. Boot-once is a
            // bring-up mode for a box you cannot walk to, and that is not who
            // is driving a window; the CLI has --boot-once for it.
            return hc800::install_grub(ssh, rel, false, p).map_err(|e| format!("{e:#}"));
        }
        Method::Nand => {
            return iox::install_nand(ssh, rel, p).map_err(|e| format!("{e:#}"));
        }
        _ => {}
    }

    let mut sizes = None;
    if !finish_only {
        if self_install {
            if !files.is_empty() {
                return Err("this choice needs the standard install (it writes a setting to the \
                            system partition); untick the one-shot self-install"
                    .into());
            }
            network::install_self(ssh, rel, secure_boot, p).map_err(|e| format!("{e:#}"))?;
            return Ok(());
        }
        sizes = network::stage1(ssh, rel, secure_boot, p).map_err(|e| format!("{e:#}"))?;
        s.lock().unwrap().stage1_done = true;
    }

    {
        let mut g = s.lock().unwrap();
        g.phase = Some(Phase::Restarting);
        g.wait = Some((Instant::now(), Instant::now() + Duration::from_secs(REBOOT_WAIT)));
    }
    p.emit(Event::Step("waiting for the controller to restart".into()));
    let ssh2 = tp::wait_for_login(host, REBOOT_WAIT).ok_or_else(|| {
        format!(
            "The controller did not come back within {REBOOT_WAIT} seconds. It is part-way \
             through — leave it powered, wait for it to appear on the network, then press \
             Finish install."
        )
    })?;

    {
        let mut g = s.lock().unwrap();
        g.phase = Some(Phase::Finishing);
        g.wait = None;
    }
    network::stage2_write_rootfs(&ssh2, rel, sizes, files, p).map_err(|e| format!("{e:#}"))
}

/// After openHC is in: download openhc-control4 and, where the box side runs,
/// find the box again once it has rebooted (by MAC, if its lease changed) and
/// install it. Returns where the Composer driver was saved.
fn run_control4(
    p: &Progress,
    host: &str,
    mac: Option<&str>,
    board: &'static Board,
    on_box: bool,
    zigbee: Zigbee,
) -> Result<PathBuf, String> {
    let b = control4::fetch(&control4::driver_dir(), board, on_box, p).map_err(|e| format!("{e:#}"))?;
    if !on_box {
        return Ok(b.driver);
    }
    let (_, ssh) = control4::find_again(host, mac, REBOOT_WAIT, p).ok_or_else(|| {
        "openHC did not come back on the network in time. Once it is up, run \
         `ohc-flash control4 <address>`."
            .to_string()
    })?;
    control4::install(&ssh, &b, zigbee, p).map_err(|e| format!("{e:#}"))?;
    Ok(b.driver)
}

/// Open a folder in the platform's file manager.
fn reveal(dir: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(dir).spawn();
}

// ------------------------------------------------------------------- helpers

/// Why the wizard cannot move on from `step`. Pure, so the rules that stop an
/// install — no board, wrong family, a bad release — are checkable without a
/// window.
fn gate(
    step: Step,
    host: &str,
    id: Option<&Identity>,
    board: Option<&'static Board>,
    chosen: bool,
    rel: Option<&RelInfo>,
) -> Option<String> {
    match step {
        Step::Find => host
            .trim()
            .is_empty()
            .then(|| "Scan, or type the controller's address".into()),
        Step::Check => {
            let id = id?;
            let b = board.or(id.board)?;
            let probe = Identity {
                board: Some(b),
                candidates: vec![b],
                running: id.running,
            version: None,
                raw: vec![],
                warning: None,
                unknown: None,
            };
            match method::choose(&probe, None).0 {
                Some(Method::Network | Method::Kexec | Method::Grub | Method::Nand | Method::Sd) => None,
                Some(other) => Some(format!("the {} method is command-line only", other.name())),
                None => Some("no install method applies to this controller".into()),
            }
        }
        Step::Images if !chosen => Some("Choose what to install".into()),
        Step::Images => match rel {
            None => Some("Waiting for the image".into()),
            Some(r) if !r.problems.is_empty() => Some("This release cannot be installed".into()),
            Some(_) => None,
        },
        Step::Install => None,
    }
}

fn running_text(r: Running) -> &'static str {
    match r {
        Running::Stock => "the original Control4 software",
        Running::Openhc => "openHC (already installed)",
        Running::Cefdk => "no operating system (bootloader only)",
        Running::Unknown => "unknown",
    }
}

/// The passwords to try ahead of the built-in logins: the one the user typed,
/// if any. A trivially small helper, but it keeps the "blank means none" rule
/// in exactly one place shared by the scan probe and the connect step.
fn extra_passwords(typed: &str) -> Vec<String> {
    let t = typed.trim();
    if t.is_empty() { vec![] } else { vec![t.to_string()] }
}

fn fmt_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_for(name: &str) -> Identity {
        let b = board::by_name(name).unwrap();
        Identity { board: Some(b), candidates: vec![b], running: Running::Stock,
            version: None, raw: vec![], warning: None, unknown: None }
    }

    /// The gate is the last thing between a user and a wrong write, so it gets
    /// the test: a CA-1 must not reach the network installer the wizard drives.
    #[test]
    fn gate_stops_a_board_this_app_cannot_flash() {
        let ea = id_for("ea3-v2");
        assert!(gate(Step::Check, "1.2.3.4", Some(&ea), ea.board, false, None).is_none());

        let ca = id_for("ca1");
        let why = gate(Step::Check, "1.2.3.4", Some(&ca), ca.board, false, None).unwrap();
        assert!(why.contains("uboot"), "{why}");
    }

    #[test]
    fn gate_needs_a_host_then_a_clean_release() {
        assert!(gate(Step::Find, "   ", None, None, false, None).is_some());
        assert!(gate(Step::Find, "10.0.0.9", None, None, false, None).is_none());

        let bad = RelInfo {
            source: "x".into(),
            kernel_len: 0,
            hc800: false,
            problems: vec!["no bzImage".into()],
            names: vec![],
        };
        assert!(gate(Step::Images, "h", None, None, true, Some(&bad)).is_some());
        let good = RelInfo { problems: vec![], ..bad };
        assert!(gate(Step::Images, "h", None, None, true, Some(&good)).is_none());
        assert!(gate(Step::Images, "h", None, None, false, Some(&good)).is_some());
    }

    #[test]
    fn bytes_read_like_a_person_wrote_them() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2 KB");
        assert_eq!(fmt_bytes(5_882_368), "5.6 MB");
    }

    #[test]
    fn extra_passwords_treats_blank_as_none() {
        assert!(extra_passwords("   ").is_empty());
        assert_eq!(extra_passwords("  hunter2 "), vec!["hunter2".to_string()]);
    }

    /// Lay every screen out headlessly. No GPU, no window — just proof that the
    /// whole widget tree builds without panicking, in each wizard step and each
    /// install phase, so a refactor that breaks the layout fails here instead of
    /// on a user's screen.
    #[test]
    fn every_screen_renders_without_panicking() {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(760.0, 600.0),
            )),
            ..Default::default()
        };
        let mut app = App::default();
        // Opt out of the launch auto-scan. This test is about layout, and the
        // scan spawns a live network sweep and flips `busy`, which would both
        // do real I/O from a unit test and render every later screen in the
        // busy state rather than the clean one being checked.
        app.auto_scanned = true;
        let mut pass = |app: &mut App| ctx.run_ui(input(), |ui| app.render(ui)).textures_delta.clear();
        for step in Step::ALL {
            app.step = step;
            pass(&mut app);
        }
        // The install screen has several states the happy path skips; render
        // each so a change to one is caught even with no hardware attached.
        app.step = Step::Install;
        app.forced = Some(board::by_name("ea3-v2").unwrap());
        for phase in [Phase::Writing, Phase::Restarting, Phase::Finishing, Phase::Control4, Phase::Done, Phase::Failed] {
            app.shared.lock().unwrap().phase = Some(phase);
            pass(&mut app);
        }
        // Done after the Control4 step: the Composer steps, then the failed-extension note.
        app.shared.lock().unwrap().control4_driver = Some(PathBuf::from("/tmp/openhc.c4z"));
        pass(&mut app);
        // ...and on a board that gets the driver alone (no on-screen view yet).
        app.forced = Some(board::by_name("hc250").unwrap());
        pass(&mut app);
        app.forced = Some(board::by_name("ea3-v2").unwrap());
        {
            let mut g = app.shared.lock().unwrap();
            g.control4_driver = None;
            g.control4_error = Some("no release".into());
        }
        pass(&mut app);

        // Now the update paths: a connected box plus a fetched release, so the
        // version rows, the "download latest" button and the self-update banner
        // all actually render instead of being skipped as None.
        {
            let b = board::by_name("ea3-v2").unwrap();
            let mut g = app.shared.lock().unwrap();
            g.phase = None;
            g.conn = Some(Conn {
                host: "10.0.0.9".into(),
                ssh: tp::Ssh::new("10.0.0.9", "root", None),
                id: Identity { board: Some(b), candidates: vec![b], running: Running::Openhc,
            version: None, raw: vec![], warning: None, unknown: None },
                installed_version: Some("old-dev".into()),
            });
            g.latest = Some(
                serde_json::from_str(
                    r#"{"tag_name":"v9.9.9","assets":[
                        {"name":"openhc-ea3-v2-v9.9.9.zip","browser_download_url":"http://x/a","size":1},
                        {"name":"ohc-flasher-v9.9.9-macos.zip","browser_download_url":"http://x/b","size":1},
                        {"name":"ohc-flasher-v9.9.9-windows.exe","browser_download_url":"http://x/c","size":1},
                        {"name":"ohc-flasher-v9.9.9-linux","browser_download_url":"http://x/d","size":1}
                    ]}"#,
                )
                .unwrap(),
            );
            g.checked_latest = true;
        }
        for step in [Step::Check, Step::Images] {
            app.step = step;
            pass(&mut app);
        }

        // The choices: an openHC image and no Home Assistant one, each choice
        // picked, Control4's Zigbee pick, and the assistants' Done screen.
        {
            let mut g = app.shared.lock().unwrap();
            g.img_openhc = g.latest.clone();
            g.img_ha = Some(serde_json::from_str(r#"{"tag_name":"dev","assets":[]}"#).unwrap());
        }
        app.step = Step::Images;
        for i in Install::ALL {
            app.with = Some(i);
            pass(&mut app);
        }
        app.with = Some(Install::Control4);
        app.zigbee = Zigbee::Control4;
        pass(&mut app);
        app.step = Step::Install;
        pass(&mut app);
        app.with = Some(Install::HomeAssistant);
        app.shared.lock().unwrap().phase = Some(Phase::Done);
        pass(&mut app);
    }
}
