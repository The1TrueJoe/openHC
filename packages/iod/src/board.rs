//! The IO model, parsed from /opt/ohc/board.env.
//!
//! This is the whole reason one binary runs the fleet. Every board answers the
//! same questions — how is the IO reached, how many of each thing is there —
//! and nothing above this module knows what a HC-800 or an IO Extender is.
//!
//! The governing rule, borrowed from the house UI: **a thing with nothing
//! behind it does not appear**. A count of zero is not "unknown", it is
//! "absent", and the API omits the section entirely rather than serving an
//! empty list for the UI to draw an empty panel from.
use serde::Serialize;
use std::collections::HashMap;

/// How the relays/contacts/IR are physically reached. The ONLY thing the rest
/// of the daemon branches on.
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Behind a microcontroller on a serial port, spoken to with DLE/STX.
    /// HC-800: LM3S1162 @115200. EA family: TM4C1231D5 @460800.
    Mcu,
    /// Native SoC GPIO lines (the DM355 IO Extender).
    Gpio,
    /// The board has no local IO of its own (CA-1).
    None,
}

#[derive(Serialize, Clone)]
pub struct SerialPort {
    /// Device node for a host UART, or `None` when the port is MCU-routed and
    /// only reachable through the IO protocol.
    pub dev: Option<String>,
    pub label: String,
    pub baud: u32,
    /// `host` — a real /dev/tty the kernel owns.
    /// `mcu`  — bytes travel over the IO protocol's UART opcodes; there is no
    ///          device node, which is exactly why this distinction exists.
    pub transport: &'static str,
    /// What this PORT will actually run at.
    ///
    /// Per port, not one global list, because the ceiling is a property of what
    /// is behind the connector. A host 16550A driving an RS-232 transceiver and
    /// a UART reached over the IO microcontroller's protocol do not have the
    /// same limits, and offering a rate the hardware cannot reach is offering a
    /// setting whose only effect is garbage on the wire.
    pub bauds: Vec<u32>,
}

#[derive(Serialize, Clone)]
pub struct Io {
    pub backend: Backend,
    pub mcu_part: Option<String>,
    #[serde(skip)]
    pub mcu_tty: Option<String>,
    #[serde(skip)]
    pub mcu_baud: u32,

    /// Rear IR jacks.
    pub ir_out: u8,
    /// How many of `ir_out` can be switched to SERIAL instead. On the EA family
    /// the combo ports are the same physical connectors as the user UARTs, so
    /// these two counts share one budget: a port is IR or serial, never both.
    pub ir_combo: u8,
    /// Internal front blaster, counted separately from the rear jacks.
    pub ir_blaster: u8,
    /// Front receiver, for learning.
    pub ir_in: u8,
    pub relays: u8,
    pub contacts: u8,

    /// Native GPIO line numbers, `gpio` backend only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relay_gpios: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contact_gpios: Vec<u32>,

    pub serials: Vec<SerialPort>,
}

#[derive(Serialize, Clone)]
pub struct Board {
    pub model: String,
    pub hostname: String,
    pub io: Io,
}

fn read_env(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Ok(s) = std::fs::read_to_string(path) else { return m };
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        // Strip a trailing comment, then quotes. board.env is shell, and every
        // one of these values is written with a comment after it.
        let v = v.split('#').next().unwrap_or("").trim();
        let v = v.trim_matches('"').trim_matches('\'');
        m.insert(k.trim().to_string(), v.to_string());
    }
    m
}

fn num<T: std::str::FromStr + Default>(e: &HashMap<String, String>, k: &str) -> T {
    e.get(k).and_then(|v| v.parse().ok()).unwrap_or_default()
}

fn lines(e: &HashMap<String, String>, k: &str) -> Vec<u32> {
    e.get(k)
        .map(|s| s.split_whitespace().filter_map(|t| t.parse().ok()).collect())
        .unwrap_or_default()
}

impl Board {
    pub fn load(env_path: &str) -> Board {
        let e = read_env(env_path);

        let backend = match e.get("OHC_IO_BACKEND").map(String::as_str) {
            Some("mcu") => Backend::Mcu,
            Some("gpio") => Backend::Gpio,
            _ => Backend::None,
        };

        // Rates a port may be set to. Overridable per transport in board.env,
        // because these are hardware facts and boards differ.
        let bauds = |key: &str, fallback: &[u32]| -> Vec<u32> {
            e.get(key)
                .map(|s| s.split_whitespace().filter_map(|t| t.parse().ok()).collect::<Vec<u32>>())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| fallback.to_vec())
        };
        // The classic RS-232 ladder, and where Control4's own configuration
        // stops. A 16550A can be divided further, but the line drivers on these
        // controllers are not specified past 115200 and nothing at the far end
        // of a serial cable expects to be.
        let host_bauds = bauds("OHC_SERIAL_BAUDS_HOST",
            &[1200, 2400, 4800, 9600, 14400, 19200, 38400, 57600, 115200]);
        // MCU-routed ports are more constrained: the rate is set by an opcode
        // the firmware interprets, not by a divisor we control. This list is
        // the conservative common set, and is UNVERIFIED against the firmware —
        // see the note in the EA board.env.
        let mcu_bauds = bauds("OHC_SERIAL_BAUDS_MCU",
            &[1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200]);

        // Host UARTs: "dev:label:baud", space separated. Labels use underscores
        // so the field itself stays whitespace-free.
        let mut serials: Vec<SerialPort> = e
            .get("OHC_SERIALS")
            .map(|s| {
                s.split_whitespace()
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        let p: Vec<&str> = t.split(':').collect();
                        SerialPort {
                            dev: Some(format!("/dev/{}", p.first().unwrap_or(&""))),
                            label: p.get(1).map(|x| x.replace('_', " ")).unwrap_or_default(),
                            baud: p.get(2).and_then(|x| x.parse().ok()).unwrap_or(115200),
                            transport: "host",
                            bauds: host_bauds.clone(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // MCU-routed user UARTs have no device node at all. They are appended
        // after the host ports so indices stay stable as boards gain host ports.
        let mcu_uarts: u8 = num(&e, "OHC_IO_UARTS");
        for i in 0..mcu_uarts {
            serials.push(SerialPort {
                dev: None,
                label: format!("Serial {}", i + 1),
                baud: 115200,
                transport: "mcu",
                bauds: mcu_bauds.clone(),
            });
        }

        Board {
            model: e.get("OHC_MODEL").cloned().unwrap_or_else(|| "unknown".into()),
            hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "openhc".into()),
            io: Io {
                mcu_part: e.get("OHC_IO_MCU_PART").filter(|s| !s.is_empty()).cloned(),
                mcu_tty: e.get("OHC_IO_MCU_TTY").filter(|s| !s.is_empty()).cloned(),
                mcu_baud: e.get("OHC_IO_MCU_BAUD").and_then(|v| v.parse().ok()).unwrap_or(115200),
                ir_out: num(&e, "OHC_IR_OUT"),
                ir_combo: num(&e, "OHC_IR_COMBO"),
                ir_blaster: num(&e, "OHC_IR_BLASTER"),
                ir_in: num(&e, "OHC_IR_IN"),
                relays: num(&e, "OHC_RELAYS"),
                contacts: num(&e, "OHC_CONTACTS"),
                relay_gpios: lines(&e, "OHC_RELAY_GPIOS"),
                contact_gpios: lines(&e, "OHC_CONTACT_GPIOS"),
                serials,
                backend,
            },
        }
    }
}

impl Io {
    /// Total IR emitters the board can drive: rear jacks plus the internal
    /// blaster. Combo ports are NOT added — they are a subset of `ir_out`.
    pub fn ir_total(&self) -> u8 {
        self.ir_out + self.ir_blaster
    }
    pub fn has_any(&self) -> bool {
        self.backend != Backend::None || !self.serials.is_empty()
    }
}
