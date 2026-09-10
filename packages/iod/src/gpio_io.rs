//! Relays and contacts as GPIO lines.
//!
//! The kernel's `gpio-ohc-iomcu` driver presents the IO microcontroller as a
//! standard gpiochip, so this is all iod needs to do to reach a relay: find the
//! named line and drive it. No framing, no sequence numbers, no owning a UART.
//!
//! **Nothing here holds a line.** Each operation requests, acts and releases,
//! exactly as `gpioset` and `gpioget` do. That is deliberate and it is the whole
//! point of moving the protocol into the kernel: a daemon that claimed
//! `relay0..3` for its own lifetime would make `gpioset $(gpiofind relay0)=1`
//! fail with EBUSY, and the box would be no more open than when iod owned the
//! serial port.
use std::io;

#[cfg(target_os = "linux")]
use gpiocdev::{line::Value, Request};

/// The chip `gpio-ohc-iomcu` registers, for diagnostics only. It is NOT how the
/// lines are found — see [`present`].
pub const CHIP_LABEL: &str = "ohc-iomcu";

#[cfg(target_os = "linux")]
fn err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

/// Are this board's IO lines present?
///
/// Deliberately NOT "is there an ohc-iomcu chip". The HC and EA families get
/// these lines from that driver, which names them; the IO Extender gets the
/// same names from `gpio-line-names` in its device tree. One question, one
/// answer, and nothing here knows which kernel mechanism provided them.
///
/// False on an HC-800 also means the microcontroller is not answering: the
/// driver refuses to register a chip it cannot identify, precisely so that this
/// has a truthful answer rather than a chip whose every read is a lie.
#[cfg(target_os = "linux")]
pub fn present() -> bool {
    gpiocdev::find_named_line("relay1").is_some()
        || gpiocdev::find_named_line("contact1").is_some()
}

/// Resolve a line by the name the driver gave it (`relay0`, `contact2`).
#[cfg(target_os = "linux")]
fn find(name: &str) -> io::Result<(std::path::PathBuf, u32)> {
    let l = gpiocdev::find_named_line(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no GPIO line named {name}")))?;
    Ok((l.chip, l.info.offset))
}

/// Line names are ONE-BASED — `relay1` is the terminal labelled 1 on the back
/// of the box. Everything inside iod counts from zero, so the +1 lives here and
/// nowhere else.
pub fn relay_get(index: u8) -> io::Result<bool> {
    read(&format!("relay{}", index + 1))
}

pub fn contact_get(index: u8) -> io::Result<bool> {
    read(&format!("contact{}", index + 1))
}

#[cfg(target_os = "linux")]
fn read(name: &str) -> io::Result<bool> {
    let (chip, offset) = find(name)?;
    let req = Request::builder()
        .on_chip(chip)
        .with_consumer("iod")
        .with_line(offset)
        .as_input()
        .request()
        .map_err(err)?;
    let v = req.value(offset).map_err(err)?;
    Ok(v == Value::Active)
}

/// Drive a relay to a state.
///
/// Idempotent by construction: the kernel driver reads before it toggles,
/// because the firmware has no set opcode. Writing the value a relay already
/// has does nothing, which is what any caller of a GPIO line expects.
#[cfg(target_os = "linux")]
pub fn relay_set(index: u8, on: bool) -> io::Result<bool> {
    let (chip, offset) = find(&format!("relay{}", index + 1))?;
    let req = Request::builder()
        .on_chip(chip)
        .with_consumer("iod")
        .with_line(offset)
        .as_output(if on { Value::Active } else { Value::Inactive })
        .request()
        .map_err(err)?;
    // Read back rather than trusting the write: the driver returns an error if
    // the relay did not land where it was asked, but a caller wants the state.
    let v = req.value(offset).map_err(err)?;
    Ok(v == Value::Active)
}

pub fn relay_toggle(index: u8) -> io::Result<bool> {
    let now = relay_get(index)?;
    relay_set(index, !now)
}

/// Every contact, as a bitmask — the shape the rest of iod already speaks.
pub fn contacts_mask(count: u8) -> io::Result<u32> {
    let mut mask = 0u32;
    for i in 0..count {
        if contact_get(i)? {
            mask |= 1 << i;
        }
    }
    Ok(mask)
}

// The daemon only ever runs on Linux; these keep the host build honest and
// fail loudly rather than pretending a line was driven.
#[cfg(not(target_os = "linux"))]
pub fn present() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
fn read(_name: &str) -> io::Result<bool> {
    Err(io::Error::other("GPIO is only available on Linux"))
}

#[cfg(not(target_os = "linux"))]
pub fn relay_set(_index: u8, _on: bool) -> io::Result<bool> {
    Err(io::Error::other("GPIO is only available on Linux"))
}
