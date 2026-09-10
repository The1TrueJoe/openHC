//! iod — the openHC IO server.
//!
//! A CLIENT of the kernel, not an owner of hardware: relays and contacts are
//! `gpio-ohc-iomcu` GPIO lines, IR is lirc devices, and `gpioset` is a peer.
//! iod's job is the MQTT surface, the retained state and the config API.
//!
//! One binary runs the whole fleet. What differs between an HC-800, an EA3, an
//! IO Extender and a CA-1 is `/opt/ohc/board.env` and nothing else.
mod api;
mod b64;
mod board;
mod events;
mod gpio;
mod gpio_io;
mod ir;
mod led;
mod lirc;
mod mqtt;
mod ops;
mod serial;

use board::{Backend, Board};
use std::sync::Arc;

pub struct Config {
    pub board: Board,
    pub bus: events::Bus,
    /// Current settings, and the channel that tells the MQTT supervisor they
    /// changed. Held here so the REST handler that saves them does not need to
    /// know what a supervisor is.
    pub settings: std::sync::Mutex<mqtt::settings::Settings>,
    /// Fields the environment has pinned; the UI shows these as read-only
    /// rather than accepting an edit it cannot honour.
    pub pinned: Vec<String>,
    pub settings_tx: tokio::sync::watch::Sender<mqtt::settings::Mqtt>,
    /// Whether the ohc-iomcu gpiochip is present. False means no relay or
    /// contact can work — the discipline was never attached, or the part did
    /// not answer the driver.
    pub gpio_io: bool,
    /// Whether the driver registered IR nodes.
    pub ir_lirc: bool,
    /// The IO microcontroller's reset line, claimed on first use and then held.
    /// See gpio::Line — letting go of it could leave the part in reset.
    pub io_reset: std::sync::Mutex<Option<gpio::Line>>,
    /// One shared session per serial port. Opening a tty per client would give
    /// two people on the same console half the bytes each.
    pub serial: std::sync::Arc<serial::Hub>,
}

/// Keep the retained state true, including when a line was driven by something
/// that is not iod — `gpioset` is a peer, so the mirror is read back rather
/// than written from what iod last commanded.
///
/// Contacts every cycle: the driver serves those from its own cache. Relays are
/// deliberately not cached there, so they go at a tenth the rate — each one is
/// a UART round trip.
async fn poller(cfg: Arc<Config>) {
    use std::time::Duration;
    const RELAY_EVERY: u32 = 10; // × 200 ms

    let contacts = cfg.board.io.contacts;
    let relays = cfg.board.io.relays;
    let mut tick: u32 = 0;

    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        tick = tick.wrapping_add(1);

        if contacts > 0 {
            let r = tokio::task::spawn_blocking(move || crate::gpio_io::contacts_mask(contacts)).await;
            match r {
                Ok(Ok(mask)) => {
                    cfg.bus.set("mcu/link", serde_json::json!(true));
                    for i in 0..contacts {
                        cfg.bus.set(&format!("contact/{}", mqtt::topics::label(i as usize)), serde_json::json!(mask >> i & 1 == 1));
                    }
                }
                // The chip went away, or the part behind it stopped answering.
                _ => cfg.bus.set("mcu/link", serde_json::json!(false)),
            }
        }

        // LEDs change rarely and a sysfs read is nearly free, but there is no
        // reason to do it at contact cadence. Same slow tick as the relays.
        if tick % RELAY_EVERY == 1 {
            for l in led::list() {
                if let Ok(v) = led::get(&l.slug) {
                    cfg.bus.set(&format!("led/{}", l.slug), serde_json::json!(v));
                }
            }
        }

        if relays > 0 && (tick % RELAY_EVERY == 1) {
            let r = tokio::task::spawn_blocking(move || {
                (0..relays).map(crate::gpio_io::relay_get).collect::<Result<Vec<bool>, _>>()
            })
            .await;
            if let Ok(Ok(on)) = r {
                for (i, v) in on.iter().enumerate() {
                    cfg.bus.set(&format!("relay/{}", mqtt::topics::label(i)), serde_json::json!(v));
                }
            }
        }
    }
}

fn main() {
    let bind = std::env::var("IOD_BIND").unwrap_or_else(|_| "0.0.0.0:7070".into());
    let env_path = std::env::var("IOD_BOARD_ENV").unwrap_or_else(|_| "/opt/ohc/board.env".into());

    let board = Board::load(&env_path);
    eprintln!(
        "iod: board={} backend={:?} ir={}+{} relays={} contacts={} serial={}",
        board.model,
        board.io.backend,
        board.io.ir_out,
        board.io.ir_blaster,
        board.io.relays,
        board.io.contacts,
        board.io.serials.len()
    );

    // The kernel owns the protocol. iod once spoke DLE/STX itself; that path is
    // gone, because two implementations of one wire format drift apart.
    let gpio_io = gpio_io::present();
    if gpio_io {
        eprintln!("iod: relays and contacts via the kernel {} gpiochip", gpio_io::CHIP_LABEL);
    } else if board.io.backend == Backend::Mcu {
        // Not fatal: capabilities still serve, so the GUI can say what is wrong
        // rather than failing to load. But no relay or contact will work.
        eprintln!("iod: no {} gpiochip — is gpio-ohc-iomcu attached? (S12iomcu)",
                  gpio_io::CHIP_LABEL);
    }

    // Separate from the gpiochip: the driver registers lirc nodes even on a
    // board with no relays.
    let ir_lirc = lirc::present();
    if ir_lirc {
        for d in lirc::devices().iter().filter(|d| d.name.starts_with("openHC IR")) {
            eprintln!("iod: IR {} -> {}", d.name, d.path.display());
        }
    }

    if !board.io.has_any() {
        eprintln!("iod: this board declares no local IO — serving capabilities only");
    }

    let (settings, pinned) = mqtt::settings::Settings::load(&board.hostname);
    eprintln!(
        "iod: mqtt serve={} listen={} bridge={}{}",
        settings.mqtt.serve,
        settings.mqtt.listen_port,
        settings.mqtt.bridge,
        if settings.mqtt.bridge { format!(" -> {}", settings.mqtt.url) } else { String::new() },
    );
    let (settings_tx, settings_rx) = tokio::sync::watch::channel(settings.mqtt.clone());
    let cfg = Arc::new(Config {
        board,
        bus: events::Bus::new(),
        settings: std::sync::Mutex::new(settings),
        pinned,
        settings_tx,
        gpio_io,
        ir_lirc,
        io_reset: std::sync::Mutex::new(None),
        serial: std::sync::Arc::new(serial::Hub::default()),
    });

    // Single-threaded on purpose: IO-bound, and it keeps the binary small.
    // A LocalSet because the serial hub's sessions are not Send.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(async move {
        // The configured line rates, so the GUI shows a port's baud before
        // anybody opens a terminal on it.
        for (i, p) in cfg.board.io.serials.iter().enumerate() {
            let n = mqtt::topics::label(i);
            cfg.bus.set(&format!("serial/{n}/baud"), serde_json::json!(p.baud));
            cfg.bus.set(&format!("serial/{n}/viewers"), serde_json::json!(0));
        }

        // The driver enables capture itself; this only reads what it decodes.
        if cfg.board.io.ir_in > 0 {
            match lirc::find(ir::RECEIVER_NAME) {
                Some(d) => {
                    tokio::task::spawn_local(ir::receiver(cfg.clone(), d.path));
                }
                None => eprintln!(
                    "iod: this board has an IR receiver but no {:?} lirc node — no ir/front/rx will fire",
                    ir::RECEIVER_NAME
                ),
            }
        }

        if cfg.gpio_io {
            tokio::task::spawn_local(poller(cfg.clone()));
        }

        // Serve MQTT, bridge outward, or both — and restart either when the
        // settings page saves.
        tokio::task::spawn_local(mqtt::supervise(cfg.clone(), settings_rx));
        if std::env::var("IOD_TOKEN").map(|t| !t.is_empty()).unwrap_or(false) {
            eprintln!("iod: token auth ENABLED");
        }
        let app = api::router(cfg);
        let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
            eprintln!("iod: cannot bind {bind}: {e}");
            std::process::exit(1);
        });
        eprintln!("iod: listening on {bind}");
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("iod: shutting down");
        };
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
            eprintln!("iod: server error: {e}");
        }
    }));
}

