//! Combo jacks: one connector that is an IR output or an MCU serial port, never
//! both at once. The kernel holds the mode (/sys/class/tty/ttyOHC<n>/mode) and
//! refuses the other side while it is set; iod saves the choice and puts it
//! back at boot.
use crate::ops::Fault;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const TTY_SYSFS: &str = "/sys/class/tty";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ir,
    Serial,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Ir => "ir",
            Mode::Serial => "serial",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "ir" => Some(Mode::Ir),
            "serial" => Some(Mode::Serial),
            _ => None,
        }
    }
}

fn attr(uart: usize, name: &str) -> PathBuf {
    Path::new(TTY_SYSFS).join(format!("ttyOHC{uart}")).join(name)
}

/// The jack's mode as the kernel has it; `None` without the driver.
pub fn mode(uart: usize) -> Option<Mode> {
    std::fs::read_to_string(attr(uart, "mode")).ok().and_then(|s| Mode::parse(&s))
}

pub fn set(uart: usize, m: Mode) -> Result<(), Fault> {
    std::fs::write(attr(uart, "mode"), m.as_str()).map_err(|e| match e.raw_os_error() {
        Some(libc::EBUSY) => Fault::Busy("the serial port is open; close it first".into()),
        Some(libc::EINVAL) => Fault::Bad(format!("this connector cannot be {}", m.as_str())),
        _ => Fault::Io(format!("set jack mode: {e}")),
    })
}
