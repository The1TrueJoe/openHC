//! The IO model: board.env for what only it knows, the kernel for the rest.
//!
//! This is the whole reason one binary runs the fleet. Every board answers the
//! same questions — how is the IO reached, how many of each thing is there —
//! and nothing above this module knows what a HC-800 or an IO Extender is.
//!
//! **Behind an IO microcontroller the counts come from the kernel.** The driver
//! latched them from the board's device tree node (EA) or from the module
//! parameters S12iomcu wrote (HC-800, which has no tree), and publishes them at
//! [`IOMCU_SYSFS`]. Reading them back, rather than a second copy in board.env,
//! is what keeps the two from disagreeing. board.env's counts are only the
//! fallback for a board whose driver never attached — the HC-800 before
//! S12iomcu, where board.env is the only description there is.
//!
//! The governing rule, borrowed from the house UI: **a thing with nothing
//! behind it does not appear**. A count of zero is not "unknown", it is
//! "absent", and the API omits the section entirely rather than serving an
//! empty list for the UI to draw an empty panel from.
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// Where gpio-ohc-iomcu publishes what the part is declared to have.
pub const IOMCU_SYSFS: &str = "/sys/class/ohc/iomcu";

/// The IO microcontroller's geometry as the kernel driver latched it.
#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    pub relays: u8,
    pub contacts: u8,
    pub ir_out: u8,
    pub ir_blaster: u8,
    pub ir_in: u8,
    pub uarts: u8,
    /// `ttyS3` behind the line discipline, `serial0-0` as a serdev device.
    pub port: String,
    pub baud: u32,
    /// The part answered the driver. False is "declared but silent".
    pub online: bool,
}

impl Declared {
    /// `None` when the driver has not attached to anything.
    pub fn read(dir: &Path) -> Option<Declared> {
        let get = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_string());
        let n = |f: &str| get(f).and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
        // relays is the attribute every version of the directory has; its
        // absence means there is no directory, not a board with no relays.
        get("relays")?;
        Some(Declared {
            relays: n("relays"),
            contacts: n("contacts"),
            ir_out: n("ir_out"),
            ir_blaster: n("ir_blaster"),
            ir_in: n("ir_in"),
            uarts: n("uarts"),
            port: get("port").unwrap_or_default(),
            baud: get("baud").and_then(|v| v.parse().ok()).unwrap_or(0),
            online: get("online").as_deref() == Some("1"),
        })
    }
}

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
    /// The device node to open. `Some` for every port that is bridgeable —
    /// both a host UART (`/dev/ttyS1`) and an MCU-routed port, which the kernel
    /// line discipline now exposes as a tty (`/dev/ttyOHC0`). `None` only when
    /// a port genuinely has no node yet, which is what the terminal refuses.
    pub dev: Option<String>,
    pub label: String,
    pub baud: u32,
    /// `host` — a real /dev/tty on a 16550A the SoC owns directly.
    /// `mcu`  — a tty whose bytes travel over the IO protocol's UART opcodes,
    ///          mux'd by the ohc-iomcu line discipline. Still a real node now;
    ///          the distinction drives the UI's "via MCU" hint, not whether the
    ///          port can be opened.
    pub transport: &'static str,
    /// What this PORT will actually run at.
    ///
    /// Per port, not one global list, because the ceiling is a property of what
    /// is behind the connector. A host 16550A driving an RS-232 transceiver and
    /// a UART reached over the IO microcontroller's protocol do not have the
    /// same limits, and offering a rate the hardware cannot reach is offering a
    /// setting whose only effect is garbage on the wire.
    pub bauds: Vec<u32>,
    /// DTR and RTS are wired to the connector (a DB9), so they can be driven.
    /// A 3.5 mm jack carries TX/RX/GND only; offering line control there would
    /// be a switch connected to nothing. Declared per port in OHC_SERIALS as a
    /// fourth `:modem` field.
    pub modem: bool,
}

#[derive(Serialize, Clone)]
pub struct Io {
    pub backend: Backend,
    pub mcu_part: Option<String>,
    /// The port the driver is on, as the kernel names it.
    #[serde(skip)]
    pub mcu_tty: Option<String>,
    #[serde(skip)]
    pub mcu_baud: u32,
    /// Where the counts below came from: `kernel` or `board.env`.
    #[serde(skip)]
    pub declared_by: &'static str,
    /// The kernel's word on whether the part answered; `None` when the
    /// driver has not attached.
    #[serde(skip)]
    pub mcu_online: Option<bool>,

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
    /// Does a CLOSED contact read as 0?
    ///
    /// True on native-GPIO boards, where these are pull-up inputs and closing
    /// the contact shorts it to ground. False behind an IO microcontroller,
    /// which normalises the sense in firmware before iod sees it. Getting this
    /// wrong does not fail loudly — it reports every idle contact as made.
    pub contacts_active_low: bool,

    /// The GPIO chip by DRIVER LABEL, not number — the number depends on probe
    /// order and has moved between kernels on this hardware.
    pub gpio_chip: Option<String>,
    /// Line that holds the IO microcontroller in reset. Having this is what
    /// makes a wedged MCU recoverable without a power cycle.
    pub io_reset_gpio: Option<u32>,
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
        Board::from_env(read_env(env_path), Declared::read(Path::new(IOMCU_SYSFS)))
    }

    fn from_env(e: HashMap<String, String>, kernel: Option<Declared>) -> Board {

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
                            modem: p.get(3) == Some(&"modem"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // MCU-routed user UARTs. The ohc-iomcu line discipline mux-es each one
        // onto a /dev/ttyOHC<n> tty (its own 0-based index, independent of any
        // host ports), so the bytes travel over the IO protocol's UART opcodes
        // while the port is, to everything above, an ordinary tty. They are
        // appended after the host ports so indices stay stable as boards gain
        // host ports. `transport` stays "mcu" — it is honest about the path and
        // drives the UI's "via MCU" hint — but `dev` is now Some, which is what
        // makes the terminal bridge open it like any other port.
        // Only an IO-microcontroller board takes the kernel's word: the
        // directory belongs to that driver.
        let kernel = kernel.filter(|_| backend == Backend::Mcu);
        let mcu_uarts: u8 = match &kernel {
            Some(k) => k.uarts,
            None => num(&e, "OHC_IO_UARTS"),
        };
        for i in 0..mcu_uarts {
            serials.push(SerialPort {
                dev: Some(format!("/dev/ttyOHC{i}")),
                label: format!("Serial {}", i + 1),
                baud: 115200,
                transport: "mcu",
                bauds: mcu_bauds.clone(),
                modem: false,
            });
        }

        Board {
            model: e.get("OHC_MODEL").cloned().unwrap_or_else(|| "unknown".into()),
            hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "openhc".into()),
            io: Io {
                mcu_part: e.get("OHC_IO_MCU_PART").filter(|s| !s.is_empty()).cloned(),
                mcu_tty: match &kernel {
                    Some(k) => Some(k.port.clone()).filter(|s| !s.is_empty()),
                    None => e.get("OHC_IO_MCU_TTY").filter(|s| !s.is_empty()).cloned(),
                },
                mcu_baud: match &kernel {
                    Some(k) => k.baud,
                    None => e.get("OHC_IO_MCU_BAUD").and_then(|v| v.parse().ok()).unwrap_or(115200),
                },
                declared_by: if kernel.is_some() { "kernel" } else { "board.env" },
                mcu_online: kernel.as_ref().map(|k| k.online),
                ir_out: kernel.as_ref().map_or_else(|| num(&e, "OHC_IR_OUT"), |k| k.ir_out),
                // The combo jacks ARE the MCU-routed UARTs (see Io::ir_combo),
                // so the kernel's uart count is also the combo count.
                ir_combo: kernel.as_ref().map_or_else(|| num(&e, "OHC_IR_COMBO"), |k| k.uarts),
                ir_blaster: kernel.as_ref().map_or_else(|| num(&e, "OHC_IR_BLASTER"), |k| k.ir_blaster),
                ir_in: kernel.as_ref().map_or_else(|| num(&e, "OHC_IR_IN"), |k| k.ir_in),
                relays: kernel.as_ref().map_or_else(|| num(&e, "OHC_RELAYS"), |k| k.relays),
                contacts: kernel.as_ref().map_or_else(|| num(&e, "OHC_CONTACTS"), |k| k.contacts),
                contacts_active_low: e
                    .get("OHC_CONTACTS_ACTIVE_LOW")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false),
                gpio_chip: e.get("OHC_GPIO_CHIP_LABEL").filter(|s| !s.is_empty()).cloned(),
                io_reset_gpio: e.get("OHC_GPIO_IO_RESET").and_then(|v| v.parse().ok()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn ea1() -> Declared {
        Declared {
            relays: 0, contacts: 0, ir_out: 4, ir_blaster: 1, ir_in: 1, uarts: 2,
            port: "serial0-0".into(), baud: 460800, online: true,
        }
    }

    #[test]
    fn reads_the_sysfs_directory() {
        let dir = std::env::temp_dir().join(format!("iod-iomcu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in [("relays", "0"), ("contacts", "0"), ("ir_out", "4"), ("ir_blaster", "1"),
                       ("ir_in", "1"), ("uarts", "2"), ("port", "serial0-0"), ("baud", "460800"),
                       ("online", "1")] {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
        assert_eq!(Declared::read(&dir), Some(ea1()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Declared::read(&dir), None);
    }

    #[test]
    fn kernel_counts_win_and_need_no_board_env() {
        let b = Board::from_env(env(&[("OHC_IO_BACKEND", "mcu")]), Some(ea1()));
        assert_eq!(b.io.declared_by, "kernel");
        assert_eq!((b.io.ir_out, b.io.ir_blaster, b.io.ir_in, b.io.ir_combo), (4, 1, 1, 2));
        assert_eq!((b.io.relays, b.io.contacts), (0, 0));
        assert_eq!(b.io.mcu_tty.as_deref(), Some("serial0-0"));
        assert_eq!(b.io.mcu_baud, 460800);
        assert_eq!(b.io.mcu_online, Some(true));
        let ttys: Vec<_> = b.io.serials.iter().filter_map(|p| p.dev.clone()).collect();
        assert_eq!(ttys, ["/dev/ttyOHC0", "/dev/ttyOHC1"]);
    }

    #[test]
    fn board_env_is_the_fallback_before_attach() {
        let e = env(&[("OHC_IO_BACKEND", "mcu"), ("OHC_RELAYS", "4"), ("OHC_CONTACTS", "4"),
                      ("OHC_IO_MCU_TTY", "/dev/ttyS3")]);
        let b = Board::from_env(e, None);
        assert_eq!(b.io.declared_by, "board.env");
        assert_eq!((b.io.relays, b.io.contacts), (4, 4));
        assert_eq!(b.io.mcu_online, None);
    }

    #[test]
    fn only_an_mcu_board_reads_the_driver() {
        let e = env(&[("OHC_IO_BACKEND", "gpio"), ("OHC_RELAYS", "8")]);
        let b = Board::from_env(e, Some(ea1()));
        assert_eq!(b.io.declared_by, "board.env");
        assert_eq!(b.io.relays, 8);
    }
}
