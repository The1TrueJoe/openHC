//! The IO model, read from the kernel.
//!
//! This is the whole reason one binary runs the fleet. Every board answers the
//! same questions — how is the IO reached, how many of each thing is there —
//! and nothing above this module knows what a HC-800 or an IO Extender is.
//!
//! **Everything here comes from what the board's firmware made the kernel
//! register; board.env holds software defaults only:**
//!
//! - the model is the device tree's `model`;
//! - an IO microcontroller is a serdev device whose compatible is
//!   `control4,io-mcu` (device tree on EA, an ACPI table on the HC-800), and
//!   gpio-ohc-iomcu publishes what its node declares at [`IOMCU_SYSFS`] —
//!   before the part even answers;
//! - native GPIO IO (the IO Extender) is the lines its device tree names
//!   (`relay1..N`, `contact1..N`) and the lirc devices its IR driver labels;
//! - host serial ports are the ones the board's udev rules name
//!   `/dev/ohc/serial/N` (label and DB9 modem lines as udev properties).
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
    /// The part the driver says it is (`tm4c1231d5`); empty if unknown.
    pub part: String,
    /// Each MCU UART's connector, by UART number (ttyOHC<n>).
    pub ports: Vec<UartPort>,
}

/// What the kernel says about one MCU UART's connector.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UartPort {
    /// The IR jack it shares, by panel number; 0 = a connector of its own.
    pub jack: u8,
    pub label: String,
}

impl UartPort {
    /// /sys/class/tty/ttyOHC<n>/{jack,label}, for n in 0..uarts.
    pub fn read_all(tty_dir: &Path, uarts: u8) -> Vec<UartPort> {
        (0..uarts)
            .map(|i| {
                let d = tty_dir.join(format!("ttyOHC{i}"));
                let get = |f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string());
                UartPort {
                    jack: get("jack").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
                    label: get("label").unwrap_or_default(),
                }
            })
            .collect()
    }
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
        let uarts = n("uarts");
        Some(Declared {
            relays: n("relays"),
            contacts: n("contacts"),
            ir_out: n("ir_out"),
            ir_blaster: n("ir_blaster"),
            ir_in: n("ir_in"),
            uarts,
            port: get("port").unwrap_or_default(),
            baud: get("baud").and_then(|v| v.parse().ok()).unwrap_or(0),
            online: get("online").as_deref() == Some("1"),
            part: get("part").unwrap_or_default(),
            ports: UartPort::read_all(Path::new(crate::jack::TTY_SYSFS), uarts),
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
    /// be a switch connected to nothing. Set by the board's udev rule
    /// (`OHC_SERIAL_MODEM=1`).
    pub modem: bool,
    /// The IR jack this port shares (a combo jack): it is serial only while
    /// that jack's mode is serial. `None` for a connector of its own.
    pub jack: Option<u8>,
    /// The MCU UART number (ttyOHC<n>), for the jack mode.
    #[serde(skip)]
    pub uart: Option<usize>,
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
    /// How many of `ir_out` can be switched to SERIAL instead (combo jacks, the
    /// serial ports with a `jack`): a jack is IR or serial, never both.
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

/// Where serdev devices live, bound or not; the io-mcu is one of them.
pub const SERDEV_SYSFS: &str = "/sys/bus/serial/devices";
/// The stable names the board's udev rules give its user serial ports.
pub const SERIAL_DIR: &str = "/dev/ohc/serial";
const IOMCU_COMPATIBLE: &str = "control4,io-mcu";

/// The compatibles in a device's `modalias` (`of:N<name>T<type>C<compat>…`),
/// which an ACPI PRP0001 node writes the same way; every one has a vendor
/// prefix, which the name and type pieces do not.
fn compatibles(modalias: &str) -> Vec<String> {
    let Some(m) = modalias.strip_prefix("of:") else { return vec![] };
    m.split('C').skip(1).filter(|c| c.contains(',')).map(str::to_string).collect()
}

/// A device's compatibles: its device-tree node's, else its modalias's (ACPI).
fn device_compatibles(dev: &Path) -> Vec<String> {
    match std::fs::read(dev.join("of_node/compatible")) {
        Ok(b) => b.split(|&c| c == 0).filter(|c| !c.is_empty()).map(|c| String::from_utf8_lossy(c).into_owned()).collect(),
        Err(_) => compatibles(std::fs::read_to_string(dev.join("modalias")).unwrap_or_default().trim()),
    }
}

/// The io-mcu's compatibles, if the board describes one.
fn iomcu_node(serdev: &Path) -> Option<Vec<String>> {
    std::fs::read_dir(serdev).ok()?.flatten().find_map(|d| {
        let c = device_compatibles(&d.path());
        c.iter().any(|s| s == IOMCU_COMPATIBLE).then_some(c)
    })
}

/// The part from the compatible before `control4,io-mcu`, for when the
/// driver is not bound: `control4,tm4c1231d5-io-mcu` -> `tm4c1231d5`.
fn mcu_part(compat: &[String]) -> Option<String> {
    compat.iter().filter(|c| *c != IOMCU_COMPATIBLE).find_map(|c| {
        let p = c.strip_prefix("control4,")?.strip_suffix("-io-mcu")?;
        (!p.is_empty()).then(|| p.to_string())
    })
}

/// The device tree's `model`, NUL-terminated in the blob.
pub fn dt_model(dt: &Path) -> Option<String> {
    let m = std::fs::read(dt.join("model")).ok()?;
    let m = String::from_utf8_lossy(&m).trim_end_matches('\0').trim().to_string();
    (!m.is_empty()).then_some(m)
}

/// A host user port: what `/dev/ohc/serial/N` points at, and its udev properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostPort {
    pub n: u32,
    pub dev: String,
    pub label: String,
    pub modem: bool,
}

impl HostPort {
    /// `/dev/ohc/serial/*` in panel order, minus the MCU-routed ttyOHC ports.
    pub fn discover(dir: &Path, props: impl Fn(&Path) -> String) -> Vec<HostPort> {
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut v: Vec<HostPort> = rd
            .flatten()
            .filter_map(|d| {
                let n: u32 = d.file_name().to_str()?.parse().ok()?;
                let target = std::fs::read_link(d.path()).ok()?;
                if target.file_name()?.to_str()?.starts_with("ttyOHC") {
                    return None;
                }
                Some(HostPort::from_props(n, &d.path(), &props(&d.path())))
            })
            .collect();
        v.sort_by_key(|p| p.n);
        v
    }

    /// `udevadm info -q property` output: OHC_SERIAL_LABEL, OHC_SERIAL_MODEM.
    fn from_props(n: u32, dev: &Path, props: &str) -> HostPort {
        let get = |k: &str| props.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
        HostPort {
            n,
            dev: dev.display().to_string(),
            label: get("OHC_SERIAL_LABEL").unwrap_or_else(|| format!("Serial {n}")),
            modem: get("OHC_SERIAL_MODEM").as_deref() == Some("1"),
        }
    }
}

fn udev_props(dev: &Path) -> String {
    std::process::Command::new("udevadm")
        .args(["info", "--query=property", "--name"])
        .arg(dev)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// What the kernel found, before it is turned into a [`Board`].
#[derive(Clone, Debug, Default)]
pub struct Kernel {
    pub model: Option<String>,
    pub backend: Option<Backend>,
    pub mcu_part: Option<String>,
    pub declared: Option<Declared>,
    pub hosts: Vec<HostPort>,
}

impl Kernel {
    pub fn read() -> Kernel {
        let model = dt_model(Path::new("/proc/device-tree"));
        let hosts = HostPort::discover(Path::new(SERIAL_DIR), udev_props);
        if let Some(compat) = iomcu_node(Path::new(SERDEV_SYSFS)) {
            let declared = Declared::read(Path::new(IOMCU_SYSFS));
            let part = declared.as_ref().map(|d| d.part.clone()).filter(|p| !p.is_empty());
            return Kernel {
                model,
                backend: Some(Backend::Mcu),
                mcu_part: part.or_else(|| mcu_part(&compat)),
                declared,
                hosts,
            };
        }
        let d = Declared::discover();
        let gpio = d.relays + d.contacts + d.ir_out + d.ir_blaster + d.ir_in > 0;
        Kernel {
            model,
            backend: Some(if gpio { Backend::Gpio } else { Backend::None }),
            mcu_part: None,
            declared: gpio.then_some(d),
            hosts,
        }
    }
}

impl Board {
    pub fn load(env_path: &str) -> Board {
        Board::from_env(read_env(env_path), Kernel::read())
    }

    fn from_env(e: HashMap<String, String>, k: Kernel) -> Board {
        let backend = k.backend.clone().unwrap_or(Backend::None);
        let kernel = k.declared;

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

        let mut serials: Vec<SerialPort> = k
            .hosts
            .iter()
            .map(|h| SerialPort {
                dev: Some(h.dev.clone()),
                label: h.label.clone(),
                baud: 115200,
                transport: "host",
                bauds: host_bauds.clone(),
                modem: h.modem,
                jack: None,
                uart: None,
            })
            .collect();

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
        let d = kernel.clone().unwrap_or_default();
        for i in 0..d.uarts {
            let kp = d.ports.get(i as usize).cloned().unwrap_or_default();
            serials.push(SerialPort {
                dev: Some(format!("/dev/ttyOHC{i}")),
                label: if kp.label.is_empty() { format!("Serial {}", i + 1) } else { kp.label },
                baud: 115200,
                transport: "mcu",
                bauds: mcu_bauds.clone(),
                modem: false,
                jack: Some(kp.jack).filter(|&j| j > 0),
                uart: Some(i as usize),
            });
        }
        let ir_combo = serials.iter().filter(|p| p.jack.is_some()).count() as u8;

        Board {
            model: k.model.unwrap_or_else(|| "unknown".into()),
            hostname: std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "openhc".into()),
            io: Io {
                mcu_part: k.mcu_part,
                mcu_tty: Some(d.port.clone()).filter(|s| !s.is_empty()),
                mcu_baud: d.baud,
                declared_by: "kernel",
                mcu_online: kernel.as_ref().filter(|_| mcu).map(|k| k.online),
                ir_out: d.ir_out,
                ir_combo,
                ir_blaster: d.ir_blaster,
                ir_in: d.ir_in,
                relays: d.relays,
                contacts: d.contacts,
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
            port: "serial0-0".into(), baud: 460800, online: true, part: String::new(),
            ports: vec![
                UartPort { jack: 1, label: "IR / Serial 1".into() },
                UartPort { jack: 2, label: "IR / Serial 2".into() },
            ],
        }
    }

    #[test]
    fn reads_the_sysfs_directory() {
        let dir = std::env::temp_dir().join(format!("iod-iomcu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (f, v) in [("relays", "0"), ("contacts", "0"), ("ir_out", "4"), ("ir_blaster", "1"),
                       ("ir_in", "1"), ("uarts", "2"), ("port", "serial0-0"), ("baud", "460800"),
                       ("online", "1"), ("part", "tm4c1231d5")] {
            std::fs::write(dir.join(f), format!("{v}\n")).unwrap();
        }
        // No ttyOHC* in this sandbox: the ports read back as plain connectors.
        let want = Declared { ports: vec![UartPort::default(); 2], part: "tm4c1231d5".into(), ..ea1() };
        assert_eq!(Declared::read(&dir), Some(want));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Declared::read(&dir), None);
    }

    #[test]
    fn reads_each_uart_connector() {
        let dir = std::env::temp_dir().join(format!("iod-tty-{}", std::process::id()));
        for (i, jack, label) in [(0, "1", "IR 1 / Serial 3"), (1, "0", "")] {
            let d = dir.join(format!("ttyOHC{i}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("jack"), format!("{jack}\n")).unwrap();
            std::fs::write(d.join("label"), format!("{label}\n")).unwrap();
        }
        let ports = UartPort::read_all(&dir, 2);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(ports, [
            UartPort { jack: 1, label: "IR 1 / Serial 3".into() },
            UartPort { jack: 0, label: String::new() },
        ]);
    }

    fn mcu(d: Declared) -> Kernel {
        Kernel { backend: Some(Backend::Mcu), declared: Some(d), ..Kernel::default() }
    }

    #[test]
    fn combo_jacks_come_from_the_kernel() {
        let mut d = ea1();
        d.ports[1] = UartPort::default(); // a UART on a connector of its own
        let b = Board::from_env(env(&[]), mcu(d));
        assert_eq!(b.io.ir_combo, 1);
        let s: Vec<_> = b.io.serials.iter().map(|p| (p.label.as_str(), p.jack, p.uart)).collect();
        assert_eq!(s, [("IR / Serial 1", Some(1), Some(0)), ("Serial 2", None, Some(1))]);
    }

    #[test]
    fn kernel_counts_win_and_need_no_board_env() {
        let k = Kernel { model: Some("Control4 EA-1 v2".into()), mcu_part: Some("tm4c1231d5".into()), ..mcu(ea1()) };
        let b = Board::from_env(env(&[]), k);
        assert_eq!(b.model, "Control4 EA-1 v2");
        assert_eq!(b.io.mcu_part.as_deref(), Some("tm4c1231d5"));
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
    fn hardware_lines_left_in_board_env_are_ignored() {
        // An EA board.env still carrying the old hardware lines.
        let e = env(&[("OHC_MODEL", "3"), ("OHC_IO_BACKEND", "gpio"), ("OHC_IO_MCU_PART", "lm3s1162"),
                      ("OHC_SERIALS", "ttyS1:Port:115200:modem")]);
        let b = Board::from_env(e, Kernel { model: Some("Control4 EA-3 v1".into()), ..mcu(ea1()) });
        assert_eq!(b.model, "Control4 EA-3 v1");
        assert_eq!(b.io.backend, Backend::Mcu);
        assert_eq!(b.io.mcu_part, None);
        assert!(b.io.serials.iter().all(|p| p.transport == "mcu"));
    }

    #[test]
    fn an_unbound_driver_declares_nothing() {
        let b = Board::from_env(env(&[]), Kernel { backend: Some(Backend::Mcu), ..Kernel::default() });
        assert_eq!(b.io.declared_by, "kernel");
        assert_eq!((b.io.relays, b.io.contacts, b.io.ir_out), (0, 0, 0));
        assert!(b.io.serials.is_empty());
        assert_eq!(b.io.mcu_online, None);
        assert_eq!(b.io.mcu_tty, None);
        assert_eq!(b.model, "unknown");
    }

    #[test]
    fn a_native_gpio_board_counts_what_its_kernel_named() {
        // The IO Extender: eight named relays and contacts, eight labelled
        // emitters, no blaster or receiver.
        let lirc: Vec<String> = (1..=8).map(|i| format!("openHC IR out {i}")).collect();
        let d = Declared::from_names(8, 8, &lirc);
        let b = Board::from_env(env(&[]), Kernel { backend: Some(Backend::Gpio), declared: Some(d), ..Kernel::default() });
        assert_eq!((b.io.relays, b.io.contacts, b.io.ir_out), (8, 8, 8));
        assert_eq!((b.io.ir_blaster, b.io.ir_in, b.io.ir_combo), (0, 0, 0));
        assert!(b.io.contacts_active_low);
        assert_eq!(b.io.mcu_online, None);
        assert!(b.io.serials.is_empty());
    }

    #[test]
    fn host_ports_come_from_dev_ohc_serial() {
        let dir = std::env::temp_dir().join(format!("iod-serial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (n, t) in [("2", "../../ttyS2"), ("1", "../../ttyS1"), ("3", "../../ttyOHC0")] {
            std::os::unix::fs::symlink(t, dir.join(n)).unwrap();
        }
        let props = |p: &Path| match p.file_name().unwrap().to_str().unwrap() {
            "1" => "DEVNAME=/dev/ttyS1\nOHC_SERIAL_LABEL=RS-232 port 1\nOHC_SERIAL_MODEM=1\n".to_string(),
            _ => "DEVNAME=/dev/ttyS2\n".to_string(),
        };
        let hosts = HostPort::discover(&dir, props);
        std::fs::remove_dir_all(&dir).unwrap();
        let got: Vec<_> = hosts.iter().map(|h| (h.n, h.label.as_str(), h.modem)).collect();
        assert_eq!(got, [(1, "RS-232 port 1", true), (2, "Serial 2", false)]);
        assert!(hosts[0].dev.ends_with("/1"));

        let b = Board::from_env(env(&[]), Kernel { hosts, ..Kernel::default() });
        assert_eq!(b.io.backend, Backend::None);
        assert!(b.io.has_any());
        assert_eq!(b.io.serials[0].transport, "host");
        assert!(b.io.serials[0].modem);
    }

    #[test]
    fn the_part_comes_from_the_io_mcu_compatible() {
        let c = compatibles;
        let dt = c("of:Nio-mcuT(null)Ccontrol4,tm4c1231d5-io-mcuCcontrol4,io-mcu");
        assert_eq!(dt, ["control4,tm4c1231d5-io-mcu", "control4,io-mcu"]);
        assert_eq!(mcu_part(&dt).as_deref(), Some("tm4c1231d5"));
        // An ACPI node name with a C in it is not a compatible.
        assert_eq!(c("of:NIOMCT(null)Ccontrol4,io-mcu"), ["control4,io-mcu"]);
        assert_eq!(mcu_part(&c("of:NIOMCT(null)Ccontrol4,lm3s1162-io-mcuCcontrol4,io-mcu")).as_deref(), Some("lm3s1162"));
        assert_eq!(mcu_part(&c("of:NIOMCT(null)Ccontrol4,io-mcu")), None);
        assert!(compatibles("acpi:PNP0501:").is_empty());
    }

    #[test]
    fn the_io_mcu_is_found_by_compatible_bound_or_not() {
        let dir = std::env::temp_dir().join(format!("iod-serdev-{}", std::process::id()));
        for (d, m) in [("serial0-0", "of:NbtT(null)Cbrcm,bcm43438-bt"), ("serial1-0", "of:NIOMCT(null)Ccontrol4,io-mcu")] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
            std::fs::write(dir.join(d).join("modalias"), format!("{m}\n")).unwrap();
        }
        assert_eq!(iomcu_node(&dir), Some(vec!["control4,io-mcu".to_string()]));
        // A device-tree node: its own compatible list, NUL-separated.
        std::fs::create_dir_all(dir.join("serial1-0/of_node")).unwrap();
        std::fs::write(dir.join("serial1-0/of_node/compatible"), b"control4,tm4c1231d5-io-mcu\0control4,io-mcu\0").unwrap();
        let found = iomcu_node(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, Some(vec!["control4,tm4c1231d5-io-mcu".to_string(), "control4,io-mcu".to_string()]));
        assert_eq!(iomcu_node(&dir), None);
    }

    #[test]
    fn the_model_is_the_device_tree_string() {
        let dir = std::env::temp_dir().join(format!("iod-dt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model"), b"Control4 HC-800\0").unwrap();
        assert_eq!(dt_model(&dir).as_deref(), Some("Control4 HC-800"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(dt_model(&dir), None);
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
        let b = Board::from_env(env(&[]), Kernel { backend: Some(Backend::None), declared: Some(ea1()), ..Kernel::default() });
        assert_eq!((b.io.relays, b.io.ir_out), (0, 0));
        assert!(!b.io.contacts_active_low);
    }
}
