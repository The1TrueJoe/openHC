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
//! Before arming, `stock` puts Control4's IO firmware back on the microcontroller:
//! the restored OS does not write it, and openHC cannot read the stock rootfs on
//! the NAND, so the install keeps stock's `/control4/firmware/io` tree on the SD's
//! ohcdata partition (`ohc-sd-install stock-io`). The EM357 keeps Control4's own
//! firmware throughout.
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::process::Command;

const EEPROM: &str = "/sys/bus/i2c/devices/0-0050/eeprom";
/// bootohc's try count (0 = armed, >= 3 = stock).
const TRY: u64 = 6;
/// bootohc's restore request.
const REQUEST: u64 = 7;
const RESTORE: u8 = b'R';
/// The SD's ohcdata partition and where the install keeps stock's IO tree.
const DATA: &str = "/dev/mmcblk0p4";
const DATA_MNT: &str = "/run/ohc-restore-data";
const STOCK_IO: &str = "stock-io";

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

/// Control4's IO image back, from the tree the install kept on ohcdata.
fn iomcu_from_sd() -> io::Result<String> {
    std::fs::create_dir_all(DATA_MNT)?;
    let st = Command::new("mount").args(["-t", "ext4", "-o", "ro", DATA, DATA_MNT]).status()?;
    if !st.success() {
        return Err(io::Error::other(format!("mount {DATA} failed: {st}")));
    }
    let root = std::path::Path::new(DATA_MNT).join(STOCK_IO);
    let r = if root.join("control4/firmware/io").is_dir() {
        crate::iomcu::restore_from_root(&root).map_err(io::Error::other)
    } else {
        Err(io::Error::other(format!(
            "no stock IO firmware kept on {DATA} (ohc-sd-install stock-io); --keep-iomcu restores without it"
        )))
    };
    let _ = Command::new("umount").arg(DATA_MNT).status();
    r
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
pub fn stock(reboot: bool, keep_iomcu: bool) -> io::Result<()> {
    if !booted_from_sd() {
        return Err(io::Error::other(
            "not booted from the SD install; reboot returns to stock (the request needs bootohc)",
        ));
    }
    if keep_iomcu {
        println!("stock: --keep-iomcu: the IO microcontroller keeps what it runs");
    } else {
        let m = iomcu_from_sd()?;
        println!("iomcu: {m}");
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
