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
mod gpio;
mod gpio_io;
mod link;
mod lirc;
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
    /// True when the kernel's ohc-iomcu gpiochip is present, so relays and
    /// contacts go through GPIO rather than iod speaking the wire protocol.
    pub gpio_io: bool,
    /// The kernel's lirc nodes for this board's emitters, keyed by the driver's
    /// label. Empty when the driver is not loaded, in which case IR goes down
    /// the direct MCU path.
    pub ir_lirc: bool,
    /// The IO microcontroller's reset line, claimed on first use and then held.
    /// See gpio::Line — letting go of it could leave the part in reset.
    pub io_reset: std::sync::Mutex<Option<gpio::Line>>,
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
    if cfg.gpio_io {
        poll_via_gpio(cfg).await;
    } else {
        poll_via_mcu(cfg).await;
    }
}

/// Poll through the kernel's gpiochip.
///
/// Contacts are read every cycle and cost nothing: the driver polls the
/// microcontroller once on its own work queue and serves these from its cache.
/// Relays are not cached there — a stale relay reading is worse than a slow one
/// — so they are read on a slower cadence, often enough to notice somebody
/// driving a line with `gpioset` and rarely enough not to flood a UART that
/// answers one question at a time.
async fn poll_via_gpio(cfg: Arc<Config>) {
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

/// The pre-driver path: iod speaks the wire protocol itself.
///
/// Only reached on a board whose kernel has no ohc-iomcu gpiochip. Two jobs,
/// both of which have to happen here rather than per-client:
///
/// 1. **Poll the contacts.** The MCU has no unsolicited notify for them, so
///    somebody has to ask. Doing it once here beats every client polling the
///    same single-question UART.
/// 2. **Collect what the MCU says unprompted.** An IR capture arrives because
///    a human pressed a remote, with no request to correlate it against.
///
/// 200 ms is a deliberate compromise: fast enough that a doorbell press is not
/// missed, slow enough that it does not monopolise a UART that IR transmission
/// also needs.
async fn poll_via_mcu(cfg: Arc<Config>) {
    use std::time::Duration;
    let n = cfg.board.io.contacts;
    let mut relays_known = false;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Some(l) = &cfg.link else { return };

        let (contacts, strays) = {
            let mut l = l.lock().await;
            let c = l.contacts();
            let s = l.poll(Duration::from_millis(5));
            (c, s)
        };

        match contacts {
            Ok(mask) => {
                cfg.bus.set("mcu/link", serde_json::json!(true));
                if !relays_known && cfg.board.io.relays > 0 {
                    let r = { l.lock().await.relays(cfg.board.io.relays) };
                    if let Ok(on) = r {
                        for (i, v) in on.iter().enumerate() {
                            cfg.bus.set(&format!("relay/{}", mqtt::topics::label(i)), serde_json::json!(v));
                        }
                        relays_known = true;
                    }
                }
                for i in 0..n {
                    cfg.bus.set(&format!("contact/{}", mqtt::topics::label(i as usize)), serde_json::json!(mask >> i & 1 == 1));
                }
            }
            Err(_) => {
                cfg.bus.set("mcu/link", serde_json::json!(false));
                relays_known = false;
            }
        }

        for f in strays {
            if f.opcode == mcu::OP_IRIN_CAPTURED {
                // The receiver is the FRONT one — same panel as the blaster,
                // so it shares the prefix: ir/front/send goes out, this comes
                // back.
                //
                // What the firmware hands us is NOT Pronto, and publishing it
                // as though it were would be a trap: the first word is the
                // 50 MHz timer PERIOD, not a Pronto carrier word, and every
                // mark carries bit 15. Fed straight back to ir/front/send that
                // reads as a ~3 kHz carrier and nonsense durations. So convert
                // it here, and publish something that can actually be replayed.
                let raw: Vec<u16> = f
                    .payload
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                    .collect();
                let ev = match ir_capture_to_pronto(&raw) {
                    Some((pronto, hz)) => serde_json::json!({
                        "pronto": pronto,
                        "carrier_hz": hz,
                        "durations": raw.len().saturating_sub(1),
                    }),
                    // Too short to carry a period plus a burst. Say so rather
                    // than emitting a code that cannot be replayed.
                    None => serde_json::json!({
                        "error": "capture too short to decode",
                        "words": raw.len(),
                    }),
                };
                cfg.bus.event("ir/front/rx", ev);
            }
        }
    }
}

/// Read the receiver's lirc node and publish each code as `ir/front/rx`.
///
/// mode2 is a stream of tagged 32-bit words: alternating marks and spaces in
/// microseconds, a FREQUENCY word carrying the carrier the driver measured, and
/// a TIMEOUT marking the end of a code. Accumulate until the end, then publish
/// a Pronto string — the same format `ir/front/send` accepts, so a learned code
/// can be sent straight back without any conversion in between.
///
/// Polled rather than woken: the fd is non-blocking and the kernel buffers a
/// whole burst, so a 20 ms tick costs one failed read per tick and keeps this
/// on the single runtime thread with everything else.
async fn ir_receiver(cfg: Arc<Config>, dev: std::path::PathBuf) {
    use std::io::Read;
    use std::time::Duration;

    let mut f = match lirc::open_rx(&dev) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("iod: cannot open {} for IR receive: {e}", dev.display());
            return;
        }
    };
    eprintln!("iod: IR receive on {}", dev.display());

    let mut durs: Vec<u32> = Vec::new();
    let mut carrier: u32 = 0;
    let mut buf = [0u8; 4096];
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                eprintln!("iod: IR receive stopped: {e}");
                return;
            }
        };
        let (words, _) = buf[..n].as_chunks::<4>();
        for c in words {
            let v = u32::from_ne_bytes(*c);
            let val = v & lirc::VALUE_MASK;
            match v & lirc::MODE2_MASK {
                lirc::MODE2_PULSE => durs.push(val),
                // A space before any mark is the gap since the last code, not
                // part of this one.
                0 if !durs.is_empty() => durs.push(val),
                lirc::MODE2_TIMEOUT => {
                    if !durs.is_empty() {
                        let hz = if carrier > 0 { carrier } else { 38_000 };
                        cfg.bus.event(
                            "ir/front/rx",
                            serde_json::json!({
                                "pronto": pronto_from_us(&durs, hz),
                                "carrier_hz": hz,
                                "durations": durs.len(),
                            }),
                        );
                    }
                    durs.clear();
                }
                // FREQUENCY: what the receiver measured, which the driver sends
                // ahead of every capture.
                0x0200_0000 => carrier = val,
                _ => {}
            }
        }
    }
}

/// Microsecond marks and spaces to a Pronto "0000" (learned) code.
///
/// Pronto counts in CARRIER PERIODS, not microseconds, so every duration is
/// scaled by the carrier — the one conversion that makes a learned code
/// replayable rather than merely well-formed.
fn pronto_from_us(durs: &[u32], carrier_hz: u32) -> String {
    const PRONTO_HZ: u32 = 4_145_146; // 1e6 / 0.241246
    let word = (PRONTO_HZ / carrier_hz.max(1)) as u16;
    let pairs = (durs.len() / 2) as u16;
    let mut out = format!("0000 {word:04X} {pairs:04X} 0000");
    for d in durs {
        let periods = ((*d as u64 * carrier_hz as u64) / 1_000_000).min(0x7fff) as u16;
        out.push_str(&format!(" {periods:04X}"));
    }
    out
}

/// Turn a raw IRIN capture into a Pronto code that `ir.send` will accept.
///
/// The capture is `[timer period][durations…]`, the period being the 50 MHz
/// system clock divided by the carrier, and bit 15 of each duration marking a
/// burst rather than a gap. Pronto wants a carrier WORD and plain durations, so
/// both have to be converted — and getting either wrong yields a code that is
/// accepted and radiates the wrong thing.
fn ir_capture_to_pronto(raw: &[u16]) -> Option<(String, u32)> {
    const SYS_HZ: u32 = 50_000_000;
    const PRONTO_HZ: u32 = 4_145_146; // 1e6 / 0.241246
    let period = *raw.first()? as u32;
    if period == 0 || raw.len() < 3 {
        return None;
    }
    let carrier_hz = SYS_HZ / period;
    if carrier_hz == 0 {
        return None;
    }
    let word = (PRONTO_HZ / carrier_hz) as u16;
    // Trailing zero words are padding, not a burst of length zero.
    let durs: Vec<u16> = raw[1..]
        .iter()
        .map(|d| d & 0x7fff)
        .take_while(|d| *d != 0)
        .collect();
    if durs.is_empty() {
        return None;
    }
    let pairs = (durs.len() / 2) as u16;
    let mut out = format!("0000 {word:04X} {pairs:04X} 0000");
    for d in &durs {
        out.push_str(&format!(" {d:04X}"));
    }
    Some((out, carrier_hz))
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

    // Prefer the kernel. When gpio-ohc-iomcu has registered a chip, the
    // protocol lives there and iod is a client of it like anything else; the
    // direct serial path below is what a board running an older kernel falls
    // back to, and it is on its way out.
    let gpio_io = gpio_io::present();
    if gpio_io {
        eprintln!("iod: relays and contacts via the kernel {} gpiochip", gpio_io::CHIP_LABEL);
    } else if board.io.backend == Backend::Mcu {
        eprintln!("iod: no {} gpiochip — talking to the MCU directly (deprecated path)",
                  gpio_io::CHIP_LABEL);
    }

    // IR is a separate question from the gpiochip: the driver registers lirc
    // nodes even for a board with no relays, and refuses to register a gpiochip
    // for a part that is not answering.
    let ir_lirc = lirc::present();
    if ir_lirc {
        for d in lirc::devices().iter().filter(|d| d.name.starts_with("openHC IR")) {
            eprintln!("iod: IR {} -> {}", d.name, d.path.display());
        }
    }

    // When the driver owns the port, iod must NOT also open it. The line
    // discipline makes ordinary reads return -EIO by design, so a second owner
    // would get a useless handle and log a misleading "did not answer identify"
    // on every start.
    let link = if board.io.backend == Backend::Mcu && !gpio_io {
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
        gpio_io,
        ir_lirc,
        io_reset: std::sync::Mutex::new(None),
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
            let n = mqtt::topics::label(i);
            cfg.bus.set(&format!("serial/{n}/baud"), serde_json::json!(p.baud));
            cfg.bus.set(&format!("serial/{n}/viewers"), serde_json::json!(0));
        }

        // Ask the MCU to report IR it receives. Without this the receiver is
        // deaf and no ir/front/rx event can ever fire. Only possible on the direct
        // path: once the driver owns the port, enabling capture is its job.
        if cfg.board.io.ir_in > 0 {
            if cfg.ir_lirc {
                // The driver turns capture on when it registers the receiver;
                // all that is left here is to read what it decodes.
                match lirc::find(lirc::RECEIVER_NAME) {
                    Some(d) => {
                        tokio::task::spawn_local(ir_receiver(cfg.clone(), d.path));
                    }
                    None => eprintln!("iod: no {} lirc node", lirc::RECEIVER_NAME),
                }
            } else if let Some(l) = &cfg.link {
                match l.lock().await.ir_capture(true) {
                    Ok(()) => eprintln!("iod: IR receive capture enabled"),
                    Err(e) => eprintln!("iod: could not enable IR capture: {e}"),
                }
            }
        }

        // Somebody has to keep the state mirror true, and which source it reads
        // from is not the same question as whether iod holds the serial port.
        // Keying this off `link` alone meant that the moment the driver took
        // the port — the case this whole architecture is for — the poller
        // silently never started and every relay and contact went unpublished.
        if cfg.gpio_io || cfg.link.is_some() {
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

#[cfg(test)]
mod ir_tests {
    use super::ir_capture_to_pronto;


    #[test]
    fn a_real_capture_decodes_to_a_replayable_code() {
        // Straight off the front receiver with a remote pointed at it. The
        // leading 0x0522 is the 50 MHz timer period, not a Pronto word, and the
        // marks carry bit 15 — publishing this raw as "pronto" would replay at
        // about 3 kHz instead of 38.
        let raw = [0x0522u16, 0x8062, 0x0017, 0x8018, 0x0017, 0x8030, 0x0017];
        let (code, hz) = ir_capture_to_pronto(&raw).expect("decodes");
        assert_eq!(hz, 50_000_000 / 0x0522); // 38,052 Hz — a normal remote
        // Carrier becomes a Pronto WORD, and every mark loses bit 15.
        assert!(code.starts_with("0000 006C "), "got {code}");
        assert!(code.contains(" 0062 "), "mark should lose bit 15: {code}");
        assert!(!code.contains("8062"), "bit 15 must not survive: {code}");
    }

    /// A code learned on the front receiver must go back out unchanged.
    ///
    /// Two conversions sit between: the driver turns carrier periods into
    /// microseconds for rc-core, and `pronto_from_us` turns them back. Getting
    /// either scale wrong yields a code that is well-formed and radiates the
    /// wrong thing, which no amount of type checking catches.
    #[test]
    fn a_learned_code_survives_the_round_trip() {
        let hz = 38_000u32;
        let periods = [0x0062u32, 0x0017, 0x0018, 0x0017];
        // What the driver hands rc-core.
        let us: Vec<u32> = periods.iter().map(|p| p * 1_000_000 / hz).collect();
        let code = super::pronto_from_us(&us, hz);
        let words: Vec<u16> = code
            .split_whitespace()
            .map(|w| u16::from_str_radix(w, 16).unwrap())
            .collect();
        assert_eq!(words[0], 0);
        assert_eq!(words[1], (4_145_146u32 / hz) as u16);
        assert_eq!(words[2], 2, "four durations are two pairs");
        for (got, want) in words[4..].iter().zip(periods.iter()) {
            // Integer microseconds lose a fraction of a period each way.
            assert!((*got as i32 - *want as i32).abs() <= 1, "{got:#06x} vs {want:#06x}");
        }
    }

    #[test]
    fn refuses_what_cannot_be_replayed() {
        assert!(ir_capture_to_pronto(&[]).is_none());
        assert!(ir_capture_to_pronto(&[0x0522]).is_none());     // period, no burst
        assert!(ir_capture_to_pronto(&[0, 0x8062, 0x17]).is_none()); // period 0
    }
}
