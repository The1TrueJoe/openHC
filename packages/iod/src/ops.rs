//! The command core: every IO operation, defined exactly once.
//!
//! There are three ways into this box — REST, the control WebSocket and MQTT —
//! and there must not be three implementations of "turn on relay 2". Each of
//! those is a thin adapter that parses its own wire format into a [`Cmd`],
//! calls [`dispatch`], and formats the result. Range checks, capability checks,
//! event publication and the MCU conversation all live here, so a fix applies
//! to every surface at once and no transport can quietly acquire its own rules.
use crate::{board::Backend, Config};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A failure with enough structure that each transport can render it its own
/// way: HTTP wants a status, MQTT wants a short code, the control socket wants
/// both plus a human string.
#[derive(Debug)]
pub enum Fault {
    /// The board has no such thing at all.
    NoSuch(String),
    /// The board has one, but not that index / not those arguments.
    Bad(String),
    /// This board's IO needs an MCU that is absent or unreachable.
    NoMcu,
    /// A real path that is not built yet. Distinct from NoSuch so a UI can say
    /// "not yet" instead of "you do not have this".
    Todo(String),
    /// The MCU answered wrongly, or not at all.
    Mcu(String),
    /// A local device would not open or would not talk.
    Io(String),
}

impl Fault {
    pub fn code(&self) -> &'static str {
        match self {
            Fault::NoSuch(_) => "no_such",
            Fault::Bad(_) => "bad_request",
            Fault::NoMcu => "no_mcu",
            Fault::Todo(_) => "not_implemented",
            Fault::Mcu(_) => "mcu_error",
            Fault::Io(_) => "io_error",
        }
    }
    pub fn status(&self) -> u16 {
        match self {
            Fault::NoSuch(_) => 404,
            Fault::Bad(_) => 400,
            Fault::NoMcu => 503,
            Fault::Todo(_) => 501,
            Fault::Mcu(_) => 502,
            Fault::Io(_) => 502,
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fault::NoSuch(s) | Fault::Bad(s) | Fault::Todo(s) | Fault::Mcu(s) | Fault::Io(s) => {
                write!(f, "{s}")
            }
            Fault::NoMcu => write!(f, "no IO microcontroller on this board, or its port could not be opened"),
        }
    }
}

pub type Out = Result<Value, Fault>;

/// Everything iod can be asked to do.
///
/// `#[serde(tag = "op")]` makes the control socket's wire format simply this
/// enum: `{"op":"relay.set","index":1,"on":true}`.
#[derive(Deserialize, Debug)]
#[serde(tag = "op")]
pub enum Cmd {
    #[serde(rename = "capabilities")]
    Capabilities,
    #[serde(rename = "mcu.info")]
    McuInfo,
    /// Pulse the microcontroller's reset line.
    ///
    /// Needed because the MCU CAN be wedged by a malformed request — it was,
    /// by an IR payload — and when it is, every relay, contact and IR call
    /// fails until it is reset. Without this the only cure is a power cycle,
    /// which on a controller in a rack is a site visit.
    #[serde(rename = "mcu.reset")]
    McuReset,
    #[serde(rename = "contact.get")]
    ContactGet,
    #[serde(rename = "relay.get")]
    RelayGet,
    #[serde(rename = "relay.set")]
    RelaySet { index: u8, on: bool },
    #[serde(rename = "relay.toggle")]
    RelayToggle { index: u8 },
    #[serde(rename = "ir.send")]
    IrSend {
        port: u8,
        pronto: String,
        #[serde(default = "one")]
        repeat: u8,
    },
    #[serde(rename = "serial.list")]
    SerialList,
    /// The whole state mirror. Cheap — served from memory, never the MCU.
    #[serde(rename = "state.get")]
    StateGet,
    /// Change a port's line rate. Applies to the shared session, so every
    /// viewer moves together — a serial console at two different bauds is not
    /// a thing that can exist.
    #[serde(rename = "serial.baud")]
    SerialBaud { index: usize, baud: u32 },
    /// Write to a port without holding a serial WebSocket open. Handy for
    /// automation: send one command string to a projector and walk away.
    #[serde(rename = "serial.write")]
    SerialWrite {
        index: usize,
        data: String,
        /// Interpret `data` as space-separated hex bytes instead of text.
        #[serde(default)]
        hex: bool,
        /// Interpret `data` as base64 — the same encoding `serial/N/rx` events
        /// use, so a consumer can echo back exactly what it received without
        /// having to know whether the bytes happened to be valid text.
        #[serde(default)]
        b64: bool,
    },
}
fn one() -> u8 {
    1
}

pub async fn dispatch(c: &Arc<Config>, cmd: Cmd) -> Out {
    match cmd {
        Cmd::Capabilities => Ok(capabilities(c)),
        Cmd::McuInfo => mcu_info(c).await,
        Cmd::McuReset => mcu_reset(c).await,
        Cmd::ContactGet => contacts(c).await,
        Cmd::RelayGet => relays(c).await,
        Cmd::RelaySet { index, on } => relay_write(c, index, Some(on)).await,
        Cmd::RelayToggle { index } => relay_write(c, index, None).await,
        Cmd::IrSend { port, pronto, repeat } => ir_send(c, port, &pronto, repeat).await,
        Cmd::SerialList => Ok(json!({ "serials": c.board.io.serials })),
        Cmd::StateGet => Ok(c.bus.state.doc()),
        Cmd::SerialBaud { index, baud } => serial_baud(c, index, baud),
        Cmd::SerialWrite { index, data, hex, b64 } => serial_write(c, index, &data, hex, b64),
    }
}

/// Everything a client needs to draw the UI, and nothing it does not.
///
/// A thing with nothing behind it does not appear: a board with no relays gets
/// no `relays` key at all, so a client renders straight from this document
/// without special-casing each model.
pub fn capabilities(c: &Arc<Config>) -> Value {
    let io = &c.board.io;
    let mut v = json!({
        "board":    c.board.model,
        "hostname": c.board.hostname,
        "backend":  io.backend,
        // Whether the MCU is ANSWERING, not merely whether its port opened.
        // Those differ exactly when it matters most: a wedged microcontroller
        // still has an openable tty, and reporting that as "linked" is a
        // valid-looking lie that sends people looking in the wrong place.
        "mcu_linked": c.bus.state.get("mcu/link")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| c.link.is_some()),
        // Kept separate so a client can tell "no MCU on this board" from
        // "there is one and it is not talking".
        "mcu_present": c.link.is_some(),
    });
    let m = v.as_object_mut().unwrap();
    if io.ir_total() > 0 {
        m.insert("ir".into(), json!({
            "out": io.ir_out,
            "blaster": io.ir_blaster,
            "total": io.ir_total(),
            // Combo ports share connectors with the user UARTs: a port is IR or
            // serial, never both. A client must not offer the same index twice.
            "combo": io.ir_combo,
            "receiver": io.ir_in,
        }));
    }
    if io.relays > 0 {
        m.insert("relays".into(), json!({ "count": io.relays }));
    }
    if io.contacts > 0 {
        m.insert("contacts".into(), json!({ "count": io.contacts }));
    }
    if !io.serials.is_empty() {
        // Each port carries its own permitted rates; see SerialPort::bauds.
        m.insert("serials".into(), json!(io.serials));
    }
    v
}

async fn mcu_info(c: &Arc<Config>) -> Out {
    let l = c.link.as_ref().ok_or(Fault::NoMcu)?;
    let mut l = l.lock().await;
    let (product, version) = l.identify().map_err(|e| Fault::Mcu(e.to_string()))?;
    // What the MCU says it measured, BE32. Handy as a link check: an HC-800
    // reports ~115207 against a nominal 115200.
    let measured = l.measured_baud().ok();
    Ok(json!({ "part": l.part, "baud": l.baud, "product": product,
               "version": version, "measured_baud": measured }))
}

/// Hold the MCU in reset briefly, then let it run.
///
/// Tries the polarity board.env documents first, and if the part stays silent,
/// tries the opposite before giving up. That second attempt is not superstition:
/// the HC-800's GPIO offsets were derived by translating the vendor's sysfs
/// numbers through a chip base, and the ACTIVE SENSE of the line was never
/// verified against hardware — it is a comment, not a measurement. Reporting
/// which polarity actually revived the part turns this from a guess into the
/// answer, and board.env can then be corrected.
///
/// The line is left in whichever state produced a live MCU. If neither did, it
/// is left at the documented "released" level, because a controller with a dead
/// MCU should not also be one holding it in reset.
async fn mcu_reset(c: &Arc<Config>) -> Out {
    let chip_label = c
        .board
        .io
        .gpio_chip
        .clone()
        .ok_or_else(|| Fault::NoSuch("this board declares no GPIO chip".into()))?;
    let line_no = c
        .board
        .io
        .io_reset_gpio
        .ok_or_else(|| Fault::NoSuch("this board declares no IO reset line".into()))?;

    // Claim the line once and keep it. A released line reverts to the kernel's
    // default, which for an undocumented reset pin could mean leaving the part
    // held in reset by the very call meant to revive it.
    {
        let mut held = c.io_reset.lock().unwrap();
        if held.is_none() {
            let chip = crate::gpio::find_chip(&chip_label).map_err(|e| Fault::Io(e.to_string()))?;
            *held = Some(
                crate::gpio::request_output(&chip, line_no, true)
                    .map_err(|e| Fault::Io(format!("cannot claim the reset line: {e}")))?,
            );
        }
    }

    // `released` is the level board.env says leaves the part running.
    for released in [true, false] {
        {
            let held = c.io_reset.lock().unwrap();
            let line = held.as_ref().expect("claimed above");
            line.set(!released).map_err(|e| Fault::Io(format!("cannot drive the reset line: {e}")))?;
            std::thread::sleep(Duration::from_millis(200));
            line.set(released).map_err(|e| Fault::Io(format!("cannot drive the reset line: {e}")))?;
        }
        // The part needs a moment to boot before it will answer.
        tokio::time::sleep(Duration::from_millis(600)).await;

        if let Some(l) = &c.link {
            let mut l = l.lock().await;
            for _ in 0..6 {
                if let Ok((name, ver)) = l.identify() {
                    c.bus.set("mcu/link", json!(true));
                    return Ok(json!({
                        "reset": true, "chip": chip_label, "line": line_no,
                        // The useful part: which sense actually worked.
                        "released_level": if released { 1 } else { 0 },
                        "matched_board_env": released,
                        "answering": { "product": name, "version": ver },
                    }));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    // Neither worked. Leave the pin where board.env says "running".
    if let Ok(held) = c.io_reset.lock() {
        if let Some(line) = held.as_ref() {
            let _ = line.set(true);
        }
    }
    c.bus.set("mcu/link", json!(false));
    Ok(json!({
        "reset": true, "chip": chip_label, "line": line_no, "answering": null,
        "tried": ["released=1", "released=0"],
        "note": "the part did not answer at either polarity; it may need a power cycle, \
                 or this may not be its reset line",
    }))
}

async fn contacts(c: &Arc<Config>) -> Out {
    let n = c.board.io.contacts;
    if n == 0 {
        return Err(Fault::NoSuch("this board has no contacts".into()));
    }
    match c.board.io.backend {
        Backend::Mcu => {
            let l = c.link.as_ref().ok_or(Fault::NoMcu)?;
            let mask = l.lock().await.contacts().map_err(|e| Fault::Mcu(e.to_string()))?;
            // Bit N = contact N; 1 = CLOSED.
            let closed: Vec<bool> = (0..n).map(|i| mask >> i & 1 == 1).collect();
            Ok(json!({ "mask": mask, "closed": closed }))
        }
        // The IO Extender reads its contacts as plain GPIO lines. Not wired up
        // yet; saying so beats reporting all-open, which is a valid-looking lie.
        Backend::Gpio => Err(Fault::Todo("gpio contact backend not implemented".into())),
        Backend::None => Err(Fault::NoSuch("no IO on this board".into())),
    }
}

async fn relays(c: &Arc<Config>) -> Out {
    let n = c.board.io.relays;
    if n == 0 {
        return Err(Fault::NoSuch("this board has no relays".into()));
    }
    let l = c.link.as_ref().ok_or(Fault::NoMcu)?;
    let on = l.lock().await.relays(n).map_err(|e| Fault::Mcu(e.to_string()))?;
    Ok(json!({ "count": n, "on": on }))
}

/// Set (`Some`) or toggle (`None`). One function because the only difference is
/// which link call runs — the checks and the event that follows are identical.
async fn relay_write(c: &Arc<Config>, index: u8, on: Option<bool>) -> Out {
    let n = c.board.io.relays;
    if n == 0 {
        return Err(Fault::NoSuch("this board has no relays".into()));
    }
    if index >= n {
        return Err(Fault::Bad(format!("relay {index} does not exist (0..{})", n - 1)));
    }
    let l = c.link.as_ref().ok_or(Fault::NoMcu)?;
    let r = {
        let mut l = l.lock().await;
        match on {
            Some(want) => l.relay_set(index, want),
            None => l.relay_toggle(index),
        }
    };
    let now = r.map_err(|e| Fault::Mcu(e.to_string()))?;
    // Relay position is STATE, not an event: it has a value at every instant
    // and a client that missed the change is wrong until the next one. Record
    // it so a new client is told on connect, and so the one that did not press
    // the button learns it changed.
    c.bus.set(&format!("relay/{index}"), json!(now));
    Ok(json!({ "index": index, "on": now }))
}

async fn ir_send(c: &Arc<Config>, port: u8, pronto: &str, repeat: u8) -> Out {
    let total = c.board.io.ir_total();
    if total == 0 {
        return Err(Fault::NoSuch("this board has no IR outputs".into()));
    }
    if port >= total {
        return Err(Fault::Bad(format!("port {port} out of range (0..{})", total - 1)));
    }
    let words: Result<Vec<u16>, _> =
        pronto.split_whitespace().map(|w| u16::from_str_radix(w, 16)).collect();
    let words = words.map_err(|_| Fault::Bad("pronto must be space-separated hex words".into()))?;
    if words.first() != Some(&0) {
        return Err(Fault::Bad("only Pronto code type 0000 (raw, learned) is supported".into()));
    }
    // A hard bound, learned the hard way: a 78-word code sent to the vendor
    // firmware wedged the microcontroller outright — no reply to anything,
    // through a daemon restart, until the reset line was pulsed. Whether the
    // real limit is length or a payload layout this code has wrong is not yet
    // established (see the note in the README), so this errs low: a refused
    // send is recoverable, a dead MCU needs `mcu.reset` at best.
    const MAX_WORDS: usize = 64;
    if words.len() > MAX_WORDS {
        return Err(Fault::Bad(format!(
            "code is {} words; this firmware is only known safe up to {MAX_WORDS}",
            words.len()
        )));
    }
    let l = c.link.as_ref().ok_or(Fault::NoMcu)?;

    // Payload layout is the vendor's IROUT_SEND: port, repeat, then the Pronto
    // words big-endian. Burst durations are CARRIER PERIODS, not microseconds.
    let mut payload = Vec::with_capacity(2 + words.len() * 2);
    payload.push(port);
    payload.push(repeat);
    for w in &words {
        payload.extend_from_slice(&w.to_be_bytes());
    }
    let mut l = l.lock().await;
    let f = l
        .request(crate::mcu::OP_IROUT_SEND, &payload, Duration::from_secs(3))
        .map_err(|e| Fault::Mcu(e.to_string()))?;
    Ok(json!({ "port": port, "words": words.len(), "status": f.payload }))
}

/// Resolve a serial index to a live shared session, or explain why not.
fn session(c: &Arc<Config>, index: usize) -> Result<std::sync::Arc<crate::serial::Session>, Fault> {
    let port = c
        .board
        .io
        .serials
        .get(index)
        .ok_or_else(|| Fault::NoSuch(format!("no serial port {index}")))?;
    // MCU-routed ports have no device node; their bytes travel over the IO
    // protocol's UART opcodes, which is a different path entirely.
    let dev = port.dev.clone().ok_or_else(|| {
        Fault::Todo(format!("port {index} is {}-routed; that bridge is not implemented yet", port.transport))
    })?;
    c.serial
        .session(index, &dev, port.baud, &c.bus)
        .map_err(|e| Fault::Io(format!("cannot open {dev}: {e}")))
}

fn serial_baud(c: &Arc<Config>, index: usize, baud: u32) -> Out {
    // Checked against THIS port's list. A rate a host UART reaches happily may
    // be one the IO microcontroller cannot be asked for at all.
    let port = c
        .board
        .io
        .serials
        .get(index)
        .ok_or_else(|| Fault::NoSuch(format!("no serial port {index}")))?;
    if !port.bauds.contains(&baud) {
        return Err(Fault::Bad(format!(
            "{baud} is not available on {} ({} transport); supported: {}",
            port.label,
            port.transport,
            port.bauds.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        )));
    }
    let s = session(c, index)?;
    s.set_baud(baud);
    c.bus.set(&format!("serial/{index}/baud"), json!(baud));
    Ok(json!({ "index": index, "baud": baud }))
}

fn serial_write(c: &Arc<Config>, index: usize, data: &str, hex: bool, b64: bool) -> Out {
    let bytes = if hex {
        data.split_whitespace()
            .map(|w| u8::from_str_radix(w, 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| Fault::Bad("hex must be space-separated byte values".into()))?
    } else if b64 {
        crate::b64::decode(data).ok_or_else(|| Fault::Bad("data is not valid base64".into()))?
    } else {
        data.as_bytes().to_vec()
    };
    let s = session(c, index)?;
    let n = bytes.len();
    s.write(bytes);
    Ok(json!({ "index": index, "wrote": n }))
}
