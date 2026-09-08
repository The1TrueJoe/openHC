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
mod board;
mod events;
mod link;
mod mcu;
mod serial;

use board::{Backend, Board};
use std::sync::Arc;

pub struct Config {
    pub board: Board,
    pub bus: events::Bus,
    /// `None` when the board has no MCU (gpio or none backends), or when the
    /// port could not be opened — the API still serves capabilities so the UI
    /// can say what is wrong instead of failing to load.
    pub link: Option<tokio::sync::Mutex<link::Link>>,
}

/// Poll the contact mask and publish transitions.
///
/// 200 ms is a deliberate compromise: fast enough that a doorbell press is not
/// missed, slow enough that it does not monopolise a UART that IR transmission
/// also needs. Each poll takes the same lock an API call would, so a long IR
/// burst simply delays a sample rather than corrupting one.
async fn contact_poller(cfg: Arc<Config>) {
    use std::time::Duration;
    let n = cfg.board.io.contacts;
    let mut last: Option<u32> = None;
    let mut link_up = true;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Some(l) = &cfg.link else { return };
        let r = { l.lock().await.contacts() };
        match r {
            Ok(mask) => {
                if !link_up {
                    link_up = true;
                    cfg.bus.publish(events::Event::McuLink { up: true });
                }
                if let Some(prev) = last {
                    for i in 0..n {
                        let was = prev >> i & 1 == 1;
                        let now = mask >> i & 1 == 1;
                        if was != now {
                            cfg.bus.publish(events::Event::Contact { index: i, closed: now });
                        }
                    }
                }
                last = Some(mask);
            }
            Err(_) => {
                if link_up {
                    link_up = false;
                    cfg.bus.publish(events::Event::McuLink { up: false });
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

    let cfg = Arc::new(Config { board, link, bus: events::Bus::new() });

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
        if cfg.board.io.contacts > 0 && cfg.link.is_some() {
            tokio::task::spawn_local(contact_poller(cfg.clone()));
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
