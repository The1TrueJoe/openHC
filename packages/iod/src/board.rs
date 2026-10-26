//! The IO model: board.env for what only it knows, the kernel for the rest.
//!
//! This is the whole reason one binary runs the fleet. Every board answers the
//! same questions — how is the IO reached, how many of each thing is there —
//! and nothing above this module knows what a HC-800 or an IO Extender is.
//!
//! **What the IO is comes from the kernel, never from board.env.** board.env says
//! HOW it is reached (`OHC_IO_BACKEND`); the counts are read back from what the
//! board's firmware made the kernel register:
//!
//! - behind an IO microcontroller, gpio-ohc-iomcu publishes what the part's
//!   firmware node declares (device tree on EA, an ACPI table on the HC-800) at
//!   [`IOMCU_SYSFS`] — before the part even answers;
//! - on native GPIO (the IO Extender), the lines its device tree names
//!   (`relay1..N`, `contact1..N`) and the lirc devices its IR driver labels.
//!
//! There is no second copy to disagree with: a driver that did not bind means
//! the board declares nothing, and the diagnosis says why.
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declared {
    pub relays: u8,
    pub contacts: u8,
    pub ir_out: u8,
    pub ir_blaster: u8,
    pub ir_in: u8,
    pub uarts: u8,
    /// The serdev device the part is: `serial0-0`.
    pub port: String,
    pub baud: u32,
    /// The part answered the driver. False is "declared but silent".
    pub online: bool,
}

impl Declared {
    /// A native-GPIO board's IO, as its kernel registered it: the lines its
    /// device tree names and the lirc devices its IR driver labels.
    pub fn discover() -> Declared {
        let lirc: Vec<String> = crate::lirc::devices().into_iter().map(|d| d.name).collect();
        Declared::from_names(
            crate::gpio_io::count_named("relay"),
            crate::gpio_io::count_named("contact"),
            &lirc,
        )
    }

    fn from_names(relays: u8, contacts: u8, lirc: &[String]) -> Declared {
        let has = |n: &str| lirc.iter().any(|x| x == n);
        Declared {
            relays,
            contacts,
            ir_out: (1..=24u8).take_while(|i| has(&format!("openHC IR out {i}"))).count() as u8,
            ir_blaster: has("openHC IR front blaster") as u8,
            ir_in: has(crate::ir::RECEIVER_NAME) as u8,
            ..Declared::default()
        }
    }

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
    /// both a host UART (`/dev/ttyS1`) and an MCU-routed port, which the
    /// gpio-ohc-iomcu driver exposes as a tty (`/dev/ttyOHC0`). `None` only when
    /// a port genuinely has no node yet, which is what the terminal refuses.
    pub dev: Option<String>,
    pub label: String,
    pub baud: u32,
    /// `host` — a real /dev/tty on a 16550A the SoC owns directly.
    /// `mcu`  — a tty whose bytes travel over the IO protocol's UART opcodes,
    ///          mux'd by the gpio-ohc-iomcu driver. Still a real node;
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
    /// Where the counts below came from: always the kernel now.
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
    /// Follows from the backend: true on native GPIO, where these are pull-up
    /// inputs and closing the contact shorts it to ground (measured on the IO
    /// Extender: all eight open terminals read 1); false behind an IO
    /// microcontroller, which normalises the sense in firmware before iod sees
    /// it. Getting this wrong does not fail loudly — it reports every idle
    /// contact as made.
    pub contacts_active_low: bool,

    /// The GPIO chip by DRIVER LABEL, not number — the number depends on probe
    /// order and has moved between kernels on this hardware.
    pub gpio_chip: Option<String>,
    /// Line that holds the IO microcontroller in reset. Having this is what
    /// makes a wedged MCU recoverable without a power cycle.
    pub io_reset_gpio: Option<u32>,

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

fn backend_of(e: &HashMap<String, String>) -> Backend {
    match e.get("OHC_IO_BACKEND").map(String::as_str) {
        Some("mcu") => Backend::Mcu,
        Some("gpio") => Backend::Gpio,
        _ => Backend::None,
    }
}

impl Board {
    pub fn load(env_path: &str) -> Board {
        let e = read_env(env_path);
        let kernel = match backend_of(&e) {
            Backend::Mcu => Declared::read(Path::new(IOMCU_SYSFS)),
            Backend::Gpio => Some(Declared::discover()),
            Backend::None => None,
        };
        Board::from_env(e, kernel)
    }

    fn from_env(e: HashMap<String, String>, kernel: Option<Declared>) -> Board {
        let backend = backend_of(&e);

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

        // MCU-routed user UARTs. gpio-ohc-iomcu mux-es each one
        // onto a /dev/ttyOHC<n> tty (its own 0-based index, independent of any
        // host ports), so the bytes travel over the IO protocol's UART opcodes
        // while the port is, to everything above, an ordinary tty. They are
        // appended after the host ports so indices stay stable as boards gain
        // host ports. `transport` stays "mcu" — it is honest about the path and
        // drives the UI's "via MCU" hint — but `dev` is now Some, which is what
        // makes the terminal bridge open it like any other port.
        // The kernel is the only description: an unbound driver, or a board
        // with no IO, declares nothing.
        let mcu = backend == Backend::Mcu;
        let kernel = kernel.filter(|_| backend != Backend::None);
        let k = kernel.clone().unwrap_or_default();
        for i in 0..k.uarts {
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
                mcu_tty: Some(k.port.clone()).filter(|s| !s.is_empty()),
                mcu_baud: k.baud,
                declared_by: "kernel",
                mcu_online: kernel.as_ref().filter(|_| mcu).map(|k| k.online),
                ir_out: k.ir_out,
                // The combo jacks ARE the MCU-routed UARTs (see Io::ir_combo),
                // so the kernel's uart count is also the combo count.
                ir_combo: k.uarts,
                ir_blaster: k.ir_blaster,
                ir_in: k.ir_in,
                relays: k.relays,
                contacts: k.contacts,
                contacts_active_low: backend == Backend::Gpio,
                gpio_chip: e.get("OHC_GPIO_CHIP_LABEL").filter(|s| !s.is_empty()).cloned(),
                io_reset_gpio: e.get("OHC_GPIO_IO_RESET").and_then(|v| v.parse().ok()),
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
    fn an_unbound_driver_declares_nothing() {
        // Counts left in an old board.env are not a second opinion.
        let e = env(&[("OHC_IO_BACKEND", "mcu"), ("OHC_RELAYS", "4"), ("OHC_IO_UARTS", "2")]);
        let b = Board::from_env(e, None);
        assert_eq!(b.io.declared_by, "kernel");
        assert_eq!((b.io.relays, b.io.contacts, b.io.ir_out), (0, 0, 0));
        assert!(b.io.serials.is_empty());
        assert_eq!(b.io.mcu_online, None);
        assert_eq!(b.io.mcu_tty, None);
    }

    #[test]
    fn a_native_gpio_board_counts_what_its_kernel_named() {
        // The IO Extender: eight named relays and contacts, eight labelled
        // emitters, no blaster or receiver — and board.env counts ignored.
        let lirc: Vec<String> = (1..=8).map(|i| format!("openHC IR out {i}")).collect();
        let d = Declared::from_names(8, 8, &lirc);
        let e = env(&[("OHC_IO_BACKEND", "gpio"), ("OHC_RELAYS", "2")]);
        let b = Board::from_env(e, Some(d));
        assert_eq!((b.io.relays, b.io.contacts, b.io.ir_out), (8, 8, 8));
        assert_eq!((b.io.ir_blaster, b.io.ir_in, b.io.ir_combo), (0, 0, 0));
        assert!(b.io.contacts_active_low);
        assert_eq!(b.io.mcu_online, None);
        assert!(b.io.serials.is_empty());
    }

    #[test]
    fn ir_outputs_stop_at_the_first_gap() {
        let lirc = ["openHC IR out 1", "openHC IR out 2", "openHC IR out 4",
                    "openHC IR front receiver"].map(String::from);
        let d = Declared::from_names(0, 0, &lirc);
        assert_eq!((d.ir_out, d.ir_blaster, d.ir_in), (2, 0, 1));
    }

    #[test]
    fn a_board_with_no_io_declares_none() {
        let b = Board::from_env(env(&[("OHC_IO_BACKEND", "none")]), Some(ea1()));
        assert_eq!((b.io.relays, b.io.ir_out), (0, 0));
        assert!(!b.io.contacts_active_low);
    }
}
