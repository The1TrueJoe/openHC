//! HC-250 return-to-stock, run on the box itself.
//!
//! The SD install's U-Boot script (`bootohc`, board/hc250/stock-kexec/
//! ohc-boot-env.sh) checks EEPROM byte 7 on every boot: 'R' makes it clear the
//! byte, zero Control4's boot counters, and run `restoresystem`, the same script
//! the factory-restore button runs (recovery kernel and rootfs back into slot 0,
//! then `cleanboot`), to which the install appended `setenv bootmode nand;
//! saveenv`. U-Boot then boots the restored stock. openHC never writes the NAND
//! (mainline's BCH4 OOB layout is not Control4's); U-Boot does, with its own.
//!
//! The radio and the IO microcontroller are left to stock: the EM357 keeps
//! Control4's firmware, and the restored OS writes its own IO image back.
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::process::Command;

const EEPROM: &str = "/sys/bus/i2c/devices/0-0050/eeprom";
/// bootohc's try count (0 = armed, >= 3 = stock).
const TRY: u64 = 6;
/// bootohc's restore request.
const REQUEST: u64 = 7;
const RESTORE: u8 = b'R';

fn byte(off: u64) -> io::Result<u8> {
    let mut f = std::fs::File::open(EEPROM)?;
    f.seek(SeekFrom::Start(off))?;
    let mut b = [0u8; 1];
    f.read_exact(&mut b)?;
    Ok(b[0])
}

fn set_byte(off: u64, v: u8) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(EEPROM)?;
    f.seek(SeekFrom::Start(off))?;
    f.write_all(&[v])?;
    f.sync_all()?;
    drop(f);
    let got = byte(off)?;
    if got != v {
        return Err(io::Error::other(format!("EEPROM byte {off} reads back {got:#04x}, expected {v:#04x}")));
    }
    Ok(())
}

fn booted_from_sd() -> bool {
    std::fs::read_to_string("/proc/cmdline")
        .map(|c| c.split_whitespace().any(|w| w == "ohc.boot=sd"))
        .unwrap_or(false)
}

/// What the next boot will do. The `state:` line is what iod keys on.
pub fn status() -> io::Result<()> {
    let (t, r) = (byte(TRY)?, byte(REQUEST)?);
    println!("EEPROM: try {t:#04x}, restore request {r:#04x}");
    let state = if r == RESTORE {
        "restore pending (the next boot runs Control4's factory restore once)"
    } else if booted_from_sd() {
        "openHC (installed; restorable to stock)"
    } else {
        "openHC in RAM (stock is on the NAND; a reboot returns to it)"
    };
    println!("state: {state}");
    Ok(())
}

/// Ask U-Boot for Control4's factory restore on the next boot, then reboot.
/// Only from an SD install: a RAM boot's U-Boot never reads the request (and a
/// reboot already returns it to stock).
pub fn stock(reboot: bool) -> io::Result<()> {
    if !booted_from_sd() {
        return Err(io::Error::other(
            "not booted from the SD install; reboot returns to stock (the request needs bootohc)",
        ));
    }
    set_byte(REQUEST, RESTORE)?;
    println!("stock: EEPROM byte {REQUEST} = 'R' (verified)");
    if !reboot {
        println!("stock: armed; the next boot runs Control4's factory restore");
        return Ok(());
    }
    println!("stock: rebooting into Control4's factory restore (~2 min, then stock Control4)");
    let st = Command::new("reboot").status()?;
    if !st.success() {
        return Err(io::Error::other(format!("reboot failed: {st}")));
    }
    Ok(())
}

/// True when this rootfs is an HC-250 image (`board=hc250` in the release file
/// the post-build hook writes).
pub fn is_hc250() -> bool {
    std::fs::read_to_string("/etc/openhc-release")
        .map(|r| r.lines().any(|l| l.trim() == "board=hc250"))
        .unwrap_or(false)
}
