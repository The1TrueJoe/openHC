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
    /// The kernel is not offering this board's IO lines. iod has no other way
    /// to reach them; the payload says what to go and look at.
    NoChip(String),
    /// A real path that is not built yet. Distinct from NoSuch so a UI can say
    /// "not yet" instead of "you do not have this".
    Todo(String),
    /// A local device would not open or would not talk.
    Io(String),
}

impl Fault {
    pub fn code(&self) -> &'static str {
        match self {
            Fault::NoSuch(_) => "no_such",
            Fault::Bad(_) => "bad_request",
            Fault::NoChip(_) => "no_chip",
            Fault::Todo(_) => "not_implemented",
            Fault::Io(_) => "io_error",
        }
    }
    pub fn status(&self) -> u16 {
        match self {
            Fault::NoSuch(_) => 404,
            Fault::Bad(_) => 400,
            Fault::NoChip(_) => 503,
            Fault::Todo(_) => 501,
            Fault::Io(_) => 502,
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fault::NoSuch(s) | Fault::Bad(s) | Fault::Todo(s) | Fault::Io(s) => {
                write!(f, "{s}")
            }
            Fault::NoChip(s) => write!(f, "no GPIO lines named relay1/contact1 — {s}"),
        }
    }
}

pub type Out = Result<Value, Fault>;

/// Which emitter to fire. The front blaster is NOT "jack 7" — it is an internal
/// emitter with no socket on the back, and numbering it after the jacks would
/// send somebody looking for a seventh connector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IrTarget {
    /// Rear jack, as labelled: 1..=ir_out.
    Jack(u8),
    Front,
}

impl<'de> serde::Deserialize<'de> for IrTarget {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = Value::deserialize(d)?;
        if let Some(n) = v.as_u64() {
            return u8::try_from(n).map(IrTarget::Jack).map_err(|_| D::Error::custom("jack out of range"));
        }
        match v.as_str() {
            Some(s) if s.eq_ignore_ascii_case("front") => Ok(IrTarget::Front),
            Some(s) => s.parse::<u8>().map(IrTarget::Jack)
                .map_err(|_| D::Error::custom(format!("unknown IR target {s:?}; expected a jack number or \"front\""))),
            None => Err(D::Error::custom("IR target must be a jack number or \"front\"")),
        }
    }
}

impl IrTarget {
    /// How this target is written on the wire and in a topic.
    pub fn label(self) -> String {
        match self {
            IrTarget::Front => "front".into(),
            IrTarget::Jack(n) => n.to_string(),
        }
    }

    /// Validate against this board's geometry.
    fn index(self, io: &crate::board::Io) -> Result<u8, Fault> {
        match self {
            IrTarget::Front => {
                if io.ir_blaster == 0 {
                    return Err(Fault::NoSuch("this board has no front blaster".into()));
                }
                Ok(io.ir_out)
            }
            IrTarget::Jack(n) => {
                if n == 0 || n > io.ir_out {
                    return Err(Fault::Bad(format!("no IR jack {n} (1..{})", io.ir_out)));
                }
                Ok(n - 1)
            }
        }
    }
}

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
    /// Drive a front-panel LED. `level` omitted means full brightness, which
    /// is what an on/off caller wants without knowing max_brightness.
    #[serde(rename = "led.set")]
    LedSet {
        name: String,
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        level: Option<u32>,
    },
    #[serde(rename = "led.list")]
    LedList,
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
        port: IrTarget,
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
        Cmd::LedList => Ok(json!({ "leds": crate::led::list() })),
        Cmd::LedSet { name, on, level } => led_set(c, &name, on, level).await,
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
        "io_via": "gpio",
        "gpio_chip": crate::gpio_io::CHIP_LABEL,
        // ANSWERING, not merely present: a wedged part leaves the chip
        // registered, and reporting that as linked sends people to the wrong
        // place. `mcu_present` is the other half of the distinction.
        "mcu_linked": c.bus.state.get("mcu/link")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "mcu_present": c.gpio_io,
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
            // The front panel carries both, and they share a topic prefix.
            "front": {
                "send": io.ir_blaster > 0,
                "receive": io.ir_in > 0,
                "topics": { "send": "ir/front/send", "rx": "ir/front/rx" },
            },
            // Which node drives each port, so a client can say how to do the
            // same thing with ir-ctl.
            "via": if c.ir_lirc { "lirc" } else { "none" },
            "devices": crate::lirc::devices()
                .iter()
                .filter(|d| d.name.starts_with("openHC IR"))
                .map(|d| json!({
                    "name": d.name,
                    "device": d.stable().display().to_string(),
                    "node": d.path.display().to_string(),
                }))
                .collect::<Vec<_>>(),
        }));
    }
    // Sensors are whatever the kernel found, not board.env geometry — a board
    // with no hwmon simply has no key here.
    let sensors = crate::health::sensors();
    if !sensors.temps.is_empty() || !sensors.fans.is_empty() {
        m.insert("health".into(), json!(sensors));
    }
    let leds = crate::led::list();
    if !leds.is_empty() {
        // Panel LEDs are not board.env geometry — they are whatever the kernel
        // registered, so this reports what is actually there.
        m.insert("leds".into(), json!(leds));
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

/// What is carrying this board's IO. iod cannot ask the part who it is — the
/// driver owns the link — so this reports the chip and what board.env declares.
async fn mcu_info(c: &Arc<Config>) -> Out {
    Ok(json!({
        "via": "gpio",
        "chip": crate::gpio_io::CHIP_LABEL,
        "present": c.gpio_io,
        "part": c.board.io.mcu_part,
        "tty": c.board.io.mcu_tty,
        "baud": c.board.io.mcu_baud,
        "note": "the kernel driver owns the link; identify is not reachable from here",
    }))
}

/// Can this board's relays and contacts actually be reached?
///
/// Where the lines COME from differs per board and nothing below this cares;
/// only the diagnosis does, because "attach the line discipline" and "the pins
/// are still muxed to the video port" send you to very different places.
fn io_ready(c: &Arc<Config>) -> Result<(), Fault> {
    match c.board.io.backend {
        _ if c.gpio_io => Ok(()),
        Backend::None => Err(Fault::NoSuch("no IO on this board".into())),
        Backend::Mcu => Err(Fault::NoChip(format!(
            "the {} line discipline is not attached, or the microcontroller did not answer it (S12iomcu)",
            crate::gpio_io::CHIP_LABEL
        ))),
        Backend::Gpio => Err(Fault::NoChip(
            "this board names them in its device tree — check gpio-line-names, and that the pin-mux ran".into(),
        )),
    }
}

/// Set an LED and mirror the result, the same way a relay does.
async fn led_set(c: &Arc<Config>, name: &str, on: Option<bool>, level: Option<u32>) -> Out {
    let want = match (on, level) {
        (_, Some(l)) => Some(l),
        (Some(true), None) => None,     // None = full, resolved against max
        (Some(false), None) => Some(0),
        (None, None) => return Err(Fault::Bad("led.set needs `on` or `level`".into())),
    };
    let slug = name.to_string();
    let now = tokio::task::spawn_blocking(move || crate::led::set(&slug, want))
        .await
        .map_err(|e| Fault::Io(e.to_string()))?
        .map_err(|e| Fault::Io(e.to_string()))?;
    c.bus.set(&format!("led/{name}"), json!(now));
    Ok(json!({ "name": name, "brightness": now }))
}

/// Does the part behind the chip answer? A read on this chip is a round trip to
/// the microcontroller, and a wedged part leaves the chip registered with every
/// read failing.
async fn alive(c: &Arc<Config>) -> bool {
    if !c.gpio_io {
        return false;
    }
    let has_contact = c.board.io.contacts > 0;
    let has_relay = c.board.io.relays > 0;
    tokio::task::spawn_blocking(move || {
        if has_contact {
            crate::gpio_io::contact_get(0).is_ok()
        } else if has_relay {
            crate::gpio_io::relay_get(0).is_ok()
        } else {
            false
        }
    })
    .await
    .unwrap_or(false)
}

/// Hold the MCU in reset briefly, then let it run.
///
/// Tries both polarities: the HC-800's active sense was translated from the
/// vendor's sysfs numbers and never verified against hardware, so reporting
/// which one revived the part is how board.env gets corrected. The line is left
/// where the part came back, or at the documented "released" level if it did
/// not — a controller with a dead MCU should not also be holding it in reset.
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
                crate::gpio::Line::request_output(&chip, line_no, true)
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

        // Through the gpiochip: the same path every relay and contact read
        // takes, so a pass means the thing callers use is working.
        for _ in 0..6 {
            if alive(c).await {
                c.bus.set("mcu/link", json!(true));
                return Ok(json!({
                    "reset": true, "chip": chip_label, "line": line_no,
                    // The useful part: which sense actually worked.
                    "released_level": if released { 1 } else { 0 },
                    "matched_board_env": released,
                    "answering": true,
                }));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
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
        "reset": true, "chip": chip_label, "line": line_no, "answering": false,
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
    io_ready(c)?;
    let mask = tokio::task::spawn_blocking(move || crate::gpio_io::contacts_mask(n))
        .await
        .map_err(|e| Fault::Io(e.to_string()))?
        .map_err(|e| Fault::Io(e.to_string()))?;
    // Bit N = contact N; 1 = CLOSED.
    let closed: Vec<bool> = (0..n).map(|i| mask >> i & 1 == 1).collect();
    Ok(json!({ "mask": mask, "closed": closed, "via": "gpio" }))
}

async fn relays(c: &Arc<Config>) -> Out {
    let n = c.board.io.relays;
    if n == 0 {
        return Err(Fault::NoSuch("this board has no relays".into()));
    }
    io_ready(c)?;
    // iod is a client of the chip, exactly as gpioget is.
    let on = tokio::task::spawn_blocking(move || {
        (0..n).map(crate::gpio_io::relay_get).collect::<Result<Vec<bool>, _>>()
    })
    .await
    .map_err(|e| Fault::Io(e.to_string()))?
    .map_err(|e| Fault::Io(e.to_string()))?;
    Ok(json!({ "count": n, "on": on, "via": "gpio" }))
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
    io_ready(c)?;
    // The kernel driver reads before it toggles, so a set is idempotent here
    // for the same reason it is for any other GPIO line.
    let now = tokio::task::spawn_blocking(move || match on {
        Some(want) => crate::gpio_io::relay_set(index, want),
        None => crate::gpio_io::relay_toggle(index),
    })
    .await
    .map_err(|e| Fault::Io(e.to_string()))?
    .map_err(|e| Fault::Io(e.to_string()))?;
    // Relay position is STATE, not an event: it has a value at every instant
    // and a client that missed the change is wrong until the next one. Record
    // it so a new client is told on connect, and so the one that did not press
    // the button learns it changed.
    c.bus.set(&format!("relay/{}", crate::mqtt::topics::label(index as usize)), json!(now));
    Ok(json!({ "index": index, "on": now }))
}

async fn ir_send(c: &Arc<Config>, target: IrTarget, pronto: &str, _repeat: u8) -> Out {
    if c.board.io.ir_total() == 0 {
        return Err(Fault::NoSuch("this board has no IR outputs".into()));
    }
    // Validate the target against the board before looking for a device, so a
    // bad jack number says "no IR jack 9" rather than "no lirc device named…".
    target.index(&c.board.io)?;
    let (carrier_hz, durations) = crate::ir::parse_pronto(pronto)?;
    let n = durations.len();
    let dev = crate::ir::emitter(target)?;
    let path = dev.path.clone();
    tokio::task::spawn_blocking(move || crate::lirc::send(&path, carrier_hz, &durations))
        .await
        .map_err(|e| Fault::Io(e.to_string()))?
        .map_err(|e| Fault::Io(e.to_string()))?;
    Ok(json!({
        "target": target.label(),
        "device": dev.stable().display().to_string(),
        "name": dev.name,
        "carrier_hz": carrier_hz,
        "durations": n,
    }))
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
    c.bus.set(&format!("serial/{}/baud", crate::mqtt::topics::label(index)), json!(baud));
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
