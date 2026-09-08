//! iod — the openHC IO server.
//!
//! Owns EVERY local IO on the controller: the user serial ports, IR out and in,
//! the relays and the contacts. Nothing else opens those devices. webd and any
//! future automation talk to this over HTTP instead, which is what keeps a
//! single owner on a UART that can only answer one question at a time.
//!
//! One binary runs the whole fleet. What differs between a HC-800, an EA3, an
//! IO Extender and a CA-1 is `/opt/ohc/board.env` and nothing else.
mod api;
mod b64;
mod board;
mod events;
mod link;
mod mcu;
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
    /// One shared session per serial port. Opening a tty per client would give
    /// two people on the same console half the bytes each.
    pub serial: std::sync::Arc<serial::Hub>,
    /// `None` when the board has no MCU (gpio or none backends), or when the
    /// port could not be opened — the API still serves capabilities so the UI
    /// can say what is wrong instead of failing to load.
    pub link: Option<tokio::sync::Mutex<link::Link>>,
}

/// The one thing that talks to the MCU on its own.
///
/// Two jobs, both of which have to happen here rather than per-client:
///
/// 1. **Poll the contacts.** The MCU has no unsolicited notify for them, so
///    somebody has to ask. Doing it once here beats every client polling the
///    same single-question UART.
/// 2. **Collect what the MCU says unprompted.** An IR capture arrives because
///    a human pressed a remote, with no request to correlate it against.
///    `Link::request` matches on sequence number and sets those aside; this is
///    what turns them into events.
///
/// 200 ms is a deliberate compromise: fast enough that a doorbell press is not
/// missed, slow enough that it does not monopolise a UART that IR transmission
/// also needs. Each poll takes the same lock an API call would, so a long IR
/// burst delays a sample rather than corrupting one.
async fn poller(cfg: Arc<Config>) {
    use std::time::Duration;
    let n = cfg.board.io.contacts;
    // Relays are read ONCE here rather than every cycle. iod is the only thing
    // that can change one, so the mirror stays true without four extra MCU
    // round-trips five times a second — but it has to be seeded, or a client
    // that connects before anybody touches a relay is shown nothing at all.
    let mut relays_known = false;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Some(l) = &cfg.link else { return };

        let (contacts, strays) = {
            let mut l = l.lock().await;
            let c = l.contacts();
            // A short read with nothing pending costs nothing and is what lets
            // a remote press surface between polls.
            let s = l.poll(Duration::from_millis(5));
            (c, s)
        };

        match contacts {
            Ok(mask) => {
                cfg.bus.set("mcu/link", serde_json::json!(true));
                // Seed on the first successful poll, and again after the link
                // comes back: while it was down a relay could have been changed
                // by something we could not see, so the mirror is a guess until
                // it is re-read.
                if !relays_known && cfg.board.io.relays > 0 {
                    if let Some(l) = &cfg.link {
                        let r = { l.lock().await.relays(cfg.board.io.relays) };
                        if let Ok(on) = r {
                            for (i, v) in on.iter().enumerate() {
                                cfg.bus.set(&format!("relay/{i}"), serde_json::json!(v));
                            }
                            relays_known = true;
                        }
                    }
                }
                // Contact position is state: `set` publishes only on a real
                // change, so five polls a second do not become five messages.
                for i in 0..n {
                    cfg.bus.set(&format!("contact/{i}"), serde_json::json!(mask >> i & 1 == 1));
                }
            }
            // Every other reading is meaningless while this is false, which is
            // exactly why it is worth its own piece of state.
            Err(_) => {
                cfg.bus.set("mcu/link", serde_json::json!(false));
                relays_known = false;
            }
        }

        for f in strays {
            if f.opcode == mcu::OP_IRIN_CAPTURED {
                // A remote was pressed at the receiver. This is the event an
                // external control system most wants: it is how a physical
                // button on a handset triggers something that has nothing to do
                // with this box.
                cfg.bus.event(
                    "ir/rx",
                    serde_json::json!({
                        "pronto": f.payload.chunks(2)
                            .map(|c| format!("{:04x}", u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])))
                            .collect::<Vec<_>>().join(" "),
                        "bytes": f.payload.len(),
                    }),
                );
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

    let link = if board.io.backend == Backend::Mcu {
        match (&board.io.mcu_tty, board.io.mcu_baud) {
            (Some(tty), baud) => {
                let part = board.io.mcu_part.clone().unwrap_or_else(|| "unknown".into());
                match link::Link::open(tty, baud, &part) {
                    Ok(mut l) => {
                        match l.identify() {
                            Ok((name, ver)) => eprintln!("iod: MCU {} on {} @{} — {} {}", part, tty, baud, name, ver),
                            // Not fatal. The port opened; the part may simply be
                            // held in reset or busy, and saying so beats exiting.
                            Err(e) => eprintln!("iod: MCU on {} did not answer identify: {e}", tty),
                        }
                        Some(tokio::sync::Mutex::new(l))
                    }
                    Err(e) => {
                        eprintln!("iod: cannot open MCU port {tty}: {e}");
                        None
                    }
                }
            }
            _ => {
                eprintln!("iod: backend=mcu but OHC_IO_MCU_TTY is unset in board.env");
                None
            }
        }
    } else {
        None
    };

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
        link,
        bus: events::Bus::new(),
        settings: std::sync::Mutex::new(settings),
        pinned,
        settings_tx,
        serial: std::sync::Arc::new(serial::Hub::default()),
    });

    // Single-threaded on purpose: this daemon is IO-bound on one UART, and a
    // current-thread runtime keeps the binary small on a controller with 2 GB.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
    // A LocalSet, because the contact poller is spawned with spawn_local: the
    // MCU link is not Send-shared across threads and does not need to be.
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(async move {
        // Poll the contacts so clients can be told when one CHANGES. The MCU
        // has no unsolicited notify, so somebody has to poll; doing it once
        // here beats every client doing it separately over the same UART.
        // The configured line rates, so the GUI shows a port's baud before
        // anybody opens a terminal on it.
        for (i, p) in cfg.board.io.serials.iter().enumerate() {
            cfg.bus.set(&format!("serial/{i}/baud"), serde_json::json!(p.baud));
            cfg.bus.set(&format!("serial/{i}/viewers"), serde_json::json!(0));
        }

        if cfg.link.is_some() {
            // Ask the MCU to report IR it receives. Without this the receiver
            // is deaf and no ir/rx event can ever fire.
            if cfg.board.io.ir_in > 0 {
                if let Some(l) = &cfg.link {
                    match l.lock().await.ir_capture(true) {
                        Ok(()) => eprintln!("iod: IR receive capture enabled"),
                        Err(e) => eprintln!("iod: could not enable IR capture: {e}"),
                    }
                }
            }
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
