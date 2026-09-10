//! GPIO, through the kernel's character device.
//!
//! Uses `gpiocdev`: a maintained, pure-Rust implementation of the GPIO uAPI.
//! Pure-Rust matters here because this workspace cross-compiles with `rust-lld`
//! and no C toolchain — binding to libgpiod's C library would mean building
//! that for three targets to toggle one pin.
//!
//! It negotiates the ABI version itself, so this works on a kernel with only
//! the v2 chardev (`CONFIG_GPIO_CDEV` without `CONFIG_GPIO_CDEV_V1`) as well as
//! on one that still offers v1. That is not hypothetical: mainline defaults V1
//! off, and on such a kernel every libgpiod-1.x tool on the rootfs fails with
//! `Invalid argument` while this keeps working.
use std::io;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use gpiocdev::{line::Value, Request};

// NOTE: everything below is compiled out on the maintainer's macOS host, so a
// mistake in the Linux path shows up only in CI. That is the cost of keeping a
// fast host check; it is worth knowing rather than being surprised by.

#[cfg(target_os = "linux")]
/// Find a chip by its driver LABEL rather than its number.
///
/// board.env names `gpio_ich` because the number is not stable — it depends on
/// probe order, and the vendor's own notes record the base moving between
/// kernels. The label does not move.
pub fn find_chip(label: &str) -> io::Result<PathBuf> {
    let chips = gpiocdev::chip::chips().map_err(|e| io::Error::other(e.to_string()))?;
    for path in chips {
        let Ok(chip) = gpiocdev::Chip::from_path(&path) else { continue };
        if chip.info().map(|i| i.label == label).unwrap_or(false) {
            return Ok(path);
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("no gpiochip labelled {label}")))
}

#[cfg(target_os = "linux")]
/// One output line, claimed and held.
///
/// Held for the life of the daemon on purpose. Releasing the request hands the
/// pin back to the kernel, which returns it to its default — and for a reset
/// line whose idle level is not documented, that risks leaving the part held in
/// reset by the very call meant to revive it. Keeping the claim also means a
/// later request reuses this line instead of failing EBUSY against itself.
pub struct Line {
    req: Request,
    offset: u32,
}

#[cfg(target_os = "linux")]
impl Line {
    pub fn request_output(chip: &std::path::Path, offset: u32, initial: bool) -> io::Result<Line> {
        let req = Request::builder()
            .on_chip(chip)
            .with_consumer("iod")
            .with_line(offset)
            .as_output(level(initial))
            .request()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Line { req, offset })
    }

    pub fn set(&self, high: bool) -> io::Result<()> {
        self.req
            .set_value(self.offset, level(high))
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

#[cfg(target_os = "linux")]
fn level(high: bool) -> Value {
    if high {
        Value::Active
    } else {
        Value::Inactive
    }
}

// The daemon only ever runs on Linux. These exist so the host build stays
// useful, and they fail loudly rather than pretending to drive a pin.
#[cfg(not(target_os = "linux"))]
pub fn find_chip(_label: &str) -> io::Result<PathBuf> {
    Err(io::Error::other("GPIO is only available on Linux"))
}

#[cfg(not(target_os = "linux"))]
pub struct Line;

#[cfg(not(target_os = "linux"))]
impl Line {
    pub fn request_output(_chip: &std::path::Path, _offset: u32, _initial: bool) -> io::Result<Line> {
        Err(io::Error::other("GPIO is only available on Linux"))
    }
    pub fn set(&self, _high: bool) -> io::Result<()> {
        Err(io::Error::other("GPIO is only available on Linux"))
    }
}
