//! ohc-restore — return a Control4 controller to stock, run on the box itself.
//!
//! One tool, one interface (`status`, `stock`) for iod, the web UI's restore
//! control, the EA's front-button watcher and the flasher. The HC-800
//! has its own backend (hc800.rs: a one-shot GRUB entry into Control4's factory
//! restore); everything below this point is the EA / CEFDK one.
//!
//! openHC's install makes exactly ONE change to the boot path: it appends a
//! `script` item to the CEFDK Master Flash Header (MFH) in SPI-NOR so CEFDK runs
//! an autoscript that boots openHC instead of the stock kernel item. The stock
//! kernel item, the stock kernel, and p2's entire factory recovery payload are
//! left intact. So returning to stock is two steps:
//!
//!   1. REMOVE that MFH item  (this tool, over /dev/mtd0) — CEFDK then boots the
//!      stock kernel item again, exactly as it did from the factory;
//!   2. REIMAGE p1 from p2    — the stock rootfs. p1 is the running root, so this
//!      cannot be done in place: `stock` drops a one-shot flag and reboots, and
//!      boot-init reimages p1 before it pivots, while p1 is free.
//!
//! The MFH is SHA-256 protected and a bad write bricks past the button (recovery
//! needs an external programmer), so every write here is read-modify-ERASE-write
//! then READ-BACK-VERIFIED, and the table edit is the byte-for-byte inverse of
//! the install's (`ohc_flash_core::mfh` — the flasher's own module, unit-tested
//! there, so the install and its inverse cannot drift apart).
//!
//! Subcommands (safe -> destructive):
//!   status        parse and print the MFH table. No write.
//!   revert        remove openHC's MFH item (stock boot). Writes mtd0. REVERSIBLE
//!                 with `install` as long as p1 still holds openHC.
//!   install       re-append openHC's MFH item (undo `revert`). Writes mtd0.
//!   stock         stage the one-shot flag and reboot; boot-init reimages p1 from
//!                 p2 and reverts the MFH. The complete, one-way return to stock.
//!                 `--no-reboot` stages without rebooting.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;

use ohc_flash_core::{cefdk, mfh};
mod hc800;

const MTD: &str = "/dev/mtd0";
const ERASE_BLOCK: usize = 0x1_0000; // 64 KiB, confirmed on the S25FL127S
/// The 64 KiB erase block that contains the MFH table (0x80000 is block-aligned).
const BLOCK_OFF: u64 = mfh::TABLE_OFF as u64 & !(ERASE_BLOCK as u64 - 1);
const TBL_IN_BLK: usize = mfh::TABLE_OFF - BLOCK_OFF as usize; // 0

/// Size of openHC's autoscript content (MFH item [12] on the EA, at 0x91200).
/// Only needed to re-append it in `install`; the content itself is never touched.
const SCRIPT_LEN: u32 = 0xfa;

#[repr(C)]
struct EraseInfoUser {
    start: u32,
    length: u32,
}
// MEMERASE = _IOW('M', 2, struct erase_info_user) — 8-byte arg, x86 _IOC encoding.
// `u32` + `as _` at the call site: libc::ioctl's request arg is c_int on musl but
// c_ulong on glibc/macOS, so let the cast coerce to whatever the target wants.
const MEMERASE: u32 = 0x4008_4d02;

fn read_block_at(block_off: u64) -> io::Result<Vec<u8>> {
    let mut f = std::fs::OpenOptions::new().read(true).open(MTD)?;
    f.seek(SeekFrom::Start(block_off))?;
    let mut buf = vec![0u8; ERASE_BLOCK];
    f.read_exact(&mut buf)?;
    Ok(buf)
}
fn read_block() -> io::Result<Vec<u8>> {
    read_block_at(BLOCK_OFF)
}

/// Erase the 64 KiB block at `block_off` and write `block` back, then read it
/// back and confirm it is identical. Anything but an exact match is an error —
/// we never leave the box with a half-written block silently. Used for both the
/// MFH table block (0x80000) and the cefdk_params block (0xa0000) that holds the
/// script-enable byte.
fn erase_and_write_at(block_off: u64, block: &[u8]) -> io::Result<()> {
    assert_eq!(block.len(), ERASE_BLOCK);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(MTD)?;
    let ei = EraseInfoUser { start: block_off as u32, length: ERASE_BLOCK as u32 };
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), MEMERASE as _, &ei) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    f.seek(SeekFrom::Start(block_off))?;
    f.write_all(block)?;
    f.flush()?;
    drop(f);

    let back = read_block_at(block_off)?;
    if back != block {
        return Err(io::Error::other("read-back verify FAILED — block mismatch"));
    }
    Ok(())
}
fn erase_and_write(block: &[u8]) -> io::Result<()> {
    erase_and_write_at(BLOCK_OFF, block)
}

/// CEFDK's autoscript enable byte, inside the cefdk_params item: 0x00 = the
/// autoscript runs, 0x01 (or erased 0xff) = off. This is the byte CEFDK's own
/// `script on` / `script off` writes, and it is the AUTHORITATIVE gate on whether
/// the autoscript runs — independent of how the script MFH item was created.
/// Value mirrors `ohc_flash_core::cefdk::SCRIPT_ENABLE_OFF`.
const SCRIPT_ENABLE_OFF: u64 = 0xa0f1c;
const ENABLE_BLOCK_OFF: u64 = SCRIPT_ENABLE_OFF & !(ERASE_BLOCK as u64 - 1); // 0xa0000
const ENABLE_IN_BLK: usize = (SCRIPT_ENABLE_OFF - ENABLE_BLOCK_OFF) as usize; // 0xf1c

/// True when the autoscript is enabled (CEFDK will run it at boot).
fn script_enable_byte() -> io::Result<u8> {
    Ok(read_block_at(ENABLE_BLOCK_OFF)?[ENABLE_IN_BLK])
}

/// Clear the autoscript enable byte (0x00 -> 0x01), i.e. CEFDK `script off`, so
/// CEFDK boots the stock kernel item instead. Returns whether a change was made.
/// This is what disables the autoscript on a box whose script item was created
/// with CEFDK's own `mfh add script` (the full-MFH-table EA3 case), which the
/// MFH-table edits below never recognise.
fn set_script_enabled(on: bool) -> io::Result<bool> {
    let want = if on { 0x00 } else { 0x01 };
    let mut blk = read_block_at(ENABLE_BLOCK_OFF)?;
    if blk[ENABLE_IN_BLK] == want {
        return Ok(false);
    }
    blk[ENABLE_IN_BLK] = want;
    erase_and_write_at(ENABLE_BLOCK_OFF, &blk)?;
    Ok(true)
}

/// True when a valid MFH item is an autoscript. Two layouts exist on real boxes:
///   * an item at [`mfh::EA_CONTENT_OFF`] (0x91200): openHC's append_script item
///     (type 0, last) or a CEFDK `mfh add script` item on a full-table EA3;
///   * CEFDK's own `script on` slot at `cefdk::MFH_SCRIPT_OFF` (0x91000) of
///     exactly `cefdk::MFH_SCRIPT_LEN` (0x800) bytes, type 0x12, which pushes
///     ip_params to 0x91800 (the EA-3 v2, read 2026-10-16).
/// The size is what tells that slot apart from a virgin EA1's ip_params item,
/// which also sits at 0x91000 but is 0x10 bytes. Lets `status`/`install`
/// recognise every autoscript, so `install` never appends a duplicate.
fn script_item_present(tbl: &[u8]) -> bool {
    let rd = |o: usize| u32::from_le_bytes([tbl[o], tbl[o + 1], tbl[o + 2], tbl[o + 3]]);
    let count = (rd(mfh::COUNT_OFF) as usize).min((mfh::HASH_OFF - mfh::HDR_LEN) / mfh::ITEM_LEN);
    (0..count).any(|i| {
        let o = mfh::HDR_LEN + i * mfh::ITEM_LEN;
        let (off, len) = (rd(o + 4), rd(o + 8));
        rd(o) == mfh::FLAG_VALID
            && rd(o + 0x18) != 0xffff_ffff
            && (off == mfh::EA_CONTENT_OFF
                || (off as u64 == cefdk::MFH_SCRIPT_OFF && len as usize == cefdk::MFH_SCRIPT_LEN))
    })
}

#[cfg(test)]
mod script_item_tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()).collect()
    }

    /// The EA-3 v2's MFH table (00:0f:ff:94:ee:02, read 2026-10-16): CEFDK's
    /// `script on` item [7] at 0x91000/0x800, ip_params at 0x91800.
    const EA3_SCRIPT_ON_TABLE: &str = "0100000000000000000208000d0000000100000000000000000000000000000000000080000000000000010000000000000000000000000001000000000000000000008000f00700000800000000000000000000000000000900000000000000000000800008090008070000000000000000000000000000ffffffff00000000000000800000e000000000000000000000000000000000000200000000000000000000800000010000f0060000000000000000000000000003000000000000000000008000f8070000080000000000000000000000000000100000000000000000000080000808000000010000000000000000000000000015000000000000000000008000100900000800000000000000000000000000001200000000000000000000800018090010000000000000000000000000000000040000000000000000000080080f0a00f0050000000000000000000000000000080000000000000000000080f8140a0038d20f0000000000000000000000000005000000000000000000008000e819000000010000000000000000000000000007000000000000000000008000e81a004056680000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a6090e3258f8b46ea2d96b94692c01a985357e3002149bfdd9a22a0289c737aa";

    #[test]
    fn sees_cefdk_script_on_slot_so_install_reenables_instead_of_appending() {
        assert!(script_item_present(&hex(EA3_SCRIPT_ON_TABLE)));
    }

    #[test]
    fn ignores_ip_params_at_0x91000_on_a_virgin_table() {
        // Same table with item [7] shrunk to ip_params' 0x10 bytes.
        let mut t = hex(EA3_SCRIPT_ON_TABLE);
        let o = mfh::HDR_LEN + 7 * mfh::ITEM_LEN + 8;
        t[o..o + 4].copy_from_slice(&0x10u32.to_le_bytes());
        assert!(!script_item_present(&t));
    }

    #[test]
    fn sees_an_item_at_the_content_offset() {
        let mut t = hex(EA3_SCRIPT_ON_TABLE);
        let o = mfh::HDR_LEN + 7 * mfh::ITEM_LEN;
        t[o + 4..o + 8].copy_from_slice(&mfh::EA_CONTENT_OFF.to_le_bytes());
        t[o + 8..o + 12].copy_from_slice(&0xfau32.to_le_bytes());
        assert!(script_item_present(&t));
    }

    #[test]
    fn bzimage_offset_finds_raw_and_containered() {
        // Minimal bzImage setup header: 0x55,0xaa at 0x1fe, "HdrS" at 0x202.
        let mut bz = vec![0u8; 0x300];
        bz[0x1fe] = 0x55;
        bz[0x1ff] = 0xaa;
        bz[0x202..0x206].copy_from_slice(b"HdrS");
        // EA1: the file is the bzImage itself.
        assert_eq!(bzimage_offset(&bz), Some(0));
        // EA3: the bzImage sits behind a 0x580-byte CEFDK container header.
        let mut cont = vec![0u8; 0x580];
        cont.extend_from_slice(&bz);
        assert_eq!(bzimage_offset(&cont), Some(0x580));
        // "HdrS" without the 0x55,0xaa boot sig before it is not a kernel.
        let mut noise = vec![0u8; 0x300];
        noise[0x202..0x206].copy_from_slice(b"HdrS");
        assert_eq!(bzimage_offset(&noise), None);
        assert_eq!(bzimage_offset(&vec![0u8; 0x400]), None);
    }
}

fn table_slice(block: &mut [u8]) -> &mut [u8] {
    &mut block[TBL_IN_BLK..TBL_IN_BLK + 0x200]
}

fn cmd_status() -> io::Result<()> {
    let mut block = read_block()?;
    let tbl = table_slice(&mut block);
    let rd = |o: usize| u32::from_le_bytes([tbl[o], tbl[o + 1], tbl[o + 2], tbl[o + 3]]);
    let count = rd(mfh::COUNT_OFF) as usize;
    println!("MFH @ 0x{:x}: {} items", mfh::TABLE_OFF, count);
    for i in 0..count {
        let o = mfh::HDR_LEN + i * mfh::ITEM_LEN;
        println!(
            "  [{:2}] flags=0x{:08x} off=0x{:08x} size=0x{:<7x} type=0x{:x}",
            i, rd(o), rd(o + 4), rd(o + 8), rd(o + 0x18)
        );
    }
    let last = mfh::HDR_LEN + (count - 1) * mfh::ITEM_LEN;
    let append_item = count >= 2
        && rd(last + 0x18) == mfh::TYPE_SCRIPT
        && (0x9_0000..0xa_0000).contains(&rd(last + 4));
    let cefdk_item = script_item_present(tbl);
    let enable = script_enable_byte().unwrap_or(0xff);
    let enabled = enable == 0x00;
    println!(
        "script-enable byte @0x{:x} = 0x{:02x} ({})",
        SCRIPT_ENABLE_OFF,
        enable,
        if enabled { "autoscript ON" } else { "off" }
    );
    // The ENABLE byte is the authoritative gate on what CEFDK boots. The script
    // item (openHC-appended type-0, or a CEFDK `mfh add script` one) may linger
    // after `revert` disables the byte — re-enablable with `install`, but the box
    // boots stock meanwhile.
    let item_present = append_item || cefdk_item;
    println!(
        "state: {}",
        match (enabled, item_present) {
            (true, _) if append_item => "openHC (openHC-appended script item, enabled)",
            (true, _) => "openHC (CEFDK-created autoscript, enabled)",
            (false, true) =>
                "stock boot (autoscript DISABLED; script item still present — `install` re-enables)",
            (false, false) => "stock (no openHC autoscript)",
        }
    );
    Ok(())
}

fn cmd_revert() -> io::Result<()> {
    let mut did = false;
    // 1. Remove openHC's appended MFH script item (type-0 last item) if present.
    let mut block = read_block()?;
    match mfh::remove_script(table_slice(&mut block)) {
        Ok(removed) => {
            println!("revert: removed openHC's appended MFH item -> {removed} items; writing + verifying...");
            erase_and_write(&block)?;
            did = true;
        }
        // Expected when the autoscript was created with CEFDK's own `mfh add
        // script` (the full-MFH-table EA3 case): there is no openHC-appended item
        // to remove, and the enable byte below is the real gate.
        Err(e) => println!("revert: no openHC-appended MFH item to remove ({e})."),
    }
    // 2. Clear CEFDK's script-enable byte (= `script off`) — the authoritative
    //    gate, and the ONLY thing that disables a CEFDK-created autoscript.
    if set_script_enabled(false)? {
        println!("revert: cleared CEFDK script-enable byte @0x{SCRIPT_ENABLE_OFF:x} (autoscript OFF).");
        did = true;
    } else {
        println!("revert: script-enable byte already off.");
    }
    if did {
        println!("revert: OK — CEFDK will boot the stock kernel item. (p1 still openHC;");
        println!("        run `ohc-restore install` to undo, or `stock` to finish.)");
    } else {
        println!("revert: nothing to do — already stock.");
    }
    Ok(())
}

fn cmd_install() -> io::Result<()> {
    let mut block = read_block()?;
    // If a script item is already present (e.g. a CEFDK-created one on a
    // full-table box), don't append a duplicate — just re-enable it.
    if script_item_present(table_slice(&mut block)) {
        if set_script_enabled(true)? {
            println!("install: re-enabled the existing autoscript (script-enable byte ON).");
        } else {
            println!("install: autoscript already enabled; nothing to do.");
        }
        return Ok(());
    }
    mfh::append_script(table_slice(&mut block), mfh::EA_CONTENT_OFF, SCRIPT_LEN, mfh::TYPE_KERNEL)
        .map_err(io::Error::other)?;
    println!("install: re-appended openHC's MFH script item; writing + verifying...");
    erase_and_write(&block)?;
    set_script_enabled(true)?;
    println!("install: OK — CEFDK will boot openHC again.");
    Ok(())
}

fn cmd_stock(reboot: bool) -> io::Result<()> {
    // Restoring stock means two things: p1 must hold the factory rootfs again, and
    // the MFH autoscript (which boots openHC) must be cleared. The catch is that p1
    // cannot be reformatted while it is the running root, and clearing the autoscript
    // first would stop boot-init from ever running again — so the order depends on
    // where we are:
    //
    //   * p1 is NOT root (a ramboot, or boot-init before it pivots) — do it now:
    //     reimage p1 directly from p2, then clear the autoscript. See
    //     `cmd_reimage_finalize`.
    //
    //   * p1 IS root (a normal durable openHC boot) — we can neither reformat p1 nor
    //     kexec the recovery kernel. The EA3 fat kernel's RAM-reclaim memmap starves
    //     kexec's segment placement ("locate_hole failed"), and on every EA a kexec
    //     out of a kernel whose GPU driver has run leaves the next kernel with dead
    //     DMA and timers (see `recovery_reimage_p1`). Instead drop a one-shot flag on
    //     p1 and reboot through CEFDK with the autoscript still enabled; boot-init
    //     finds the flag before it pivots — where p1 is free — and runs the finalize
    //     there.
    if p1_is_root() {
        stage_stock_flag()?;
        println!(
            "stock: staged a one-shot restore on p1. On reboot boot-init will reimage p1 to \
             Control4 and clear the autoscript, then boot stock."
        );
        if reboot {
            println!("stock: rebooting");
            run("reboot", &[])?;
        }
        Ok(())
    } else {
        println!("stock: p1 is free — reimaging it directly from p2 and clearing the autoscript...");
        cmd_reimage_finalize()
    }
}

/// Reimage p1 to stock and clear the autoscript, in place. Only valid where p1 is
/// not the running root (a ramboot, or boot-init pre-pivot). boot-init calls this as
/// `ohc-restore reimage-finalize` after it sees the one-shot flag; `cmd_stock` calls
/// it directly when it is already running with p1 free.
fn cmd_reimage_finalize() -> io::Result<()> {
    // Refuse BEFORE touching p1 if we could not finish: the reimage succeeds without
    // the MFH, but the following revert needs /dev/mtd0, and reimaging p1 then
    // failing to clear the autoscript would leave the box with openHC destroyed, the
    // autoscript still enabled, and no staged image to reinstall from — stranded in
    // boot-init. Checking here means a transient missing mtd0 costs nothing: p1 is
    // left intact and the restore can simply be retried.
    if !std::path::Path::new(MTD).exists() {
        return Err(io::Error::other(format!(
            "{MTD} absent — refusing to reimage p1 because the autoscript (revert) could not be cleared afterwards"
        )));
    }
    reimage_p1_direct()?; // p1 -> factory rootfs (asserts p1 is not root)
    cmd_revert()?; // clear the MFH autoscript enable byte so CEFDK boots stock
    Ok(())
}

/// Drop the one-shot flag boot-init looks for before it pivots to p1. On durable,
/// p1 is `/`, so this writes it onto p1 itself; boot-init reads it from p1 and then
/// reformats p1 (destroying the flag — no restore loop). The autoscript is left
/// enabled so boot-init still runs on the next boot.
fn stage_stock_flag() -> io::Result<()> {
    std::fs::write(STOCK_FLAG, b"stock\n")?;
    run("sync", &[])?;
    Ok(())
}

/// Path of the boot-init one-shot restore flag, relative to the p1 root.
const STOCK_FLAG: &str = "/.ohc-stock-pending";

/// True when /dev/mmcblk0p1 is the filesystem mounted at `/`. We compare the st_dev
/// of `/` (the device number of the mounted root fs) against the rdev of the p1
/// device node (p1's own device number); they are equal exactly when p1 is root.
/// On a durable openHC boot p1 is root (this returns true); in a ramboot or inside
/// the boot-init bootstrap it is not (false), which is where the direct reimage is
/// allowed to run.
fn p1_is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    let root_dev = match std::fs::metadata("/") {
        Ok(m) => m.dev(),
        Err(_) => return false,
    };
    match std::fs::metadata("/dev/mmcblk0p1") {
        Ok(m) => m.rdev() == root_dev,
        Err(_) => false,
    }
}

/// Reimage p1 to stock in place, without kexec. Mount p2 read-only, empty p1's
/// existing ext4 (keeping its filesystem/LABEL/UUID, so no mke2fs and no e2fsprogs
/// in the initramfs), and stream p2's recfs.tar.xz (the factory rootfs) onto it.
/// The caller must have established that p1 is not the running root (see
/// `p1_is_root`); writing the mounted root would corrupt it.
fn reimage_p1_direct() -> io::Result<()> {
    if p1_is_root() {
        return Err(io::Error::other(
            "refusing to reimage p1: it is mounted as / (run this from a ramboot, not durable p1)",
        ));
    }
    let p2 = "/mnt/ohc-recovery";
    std::fs::create_dir_all(p2)?;
    run("mount", &["-o", "ro", "/dev/mmcblk0p2", p2])
        .or_else(|_| run("mount", &["-o", "ro,remount", "/dev/mmcblk0p2", p2]))?;
    let recfs = format!("{p2}/recfs.tar.xz");
    if !std::path::Path::new(&recfs).exists() {
        return Err(io::Error::other(format!("{recfs} missing — cannot reimage p1")));
    }

    // Reuse p1's existing ext4 rather than reformatting it: factory p1 is ext4
    // LABEL=rootfs, and emptying-then-unpacking keeps that filesystem (its type,
    // LABEL and UUID) intact — exactly what the stock kernel item expects. It also
    // avoids mke2fs, so the finalize needs no e2fsprogs in the boot-init initramfs
    // (only busybox's tar + unxz). Mount with autodetect so either ext4 or ext2 p1
    // is handled.
    let p1 = "/mnt/ohc-p1";
    std::fs::create_dir_all(p1)?;
    let _ = run("umount", &["/dev/mmcblk0p1"]); // drop any stale mount
    run("mount", &["/dev/mmcblk0p1", p1])?;
    eprintln!("stock: emptying p1 and unpacking the stock rootfs (recfs.tar.xz)...");
    // Empty p1 in place, preserving the filesystem. The glob set covers normal
    // names, dotfiles, and ..names while never matching `.`/`..`; unmatched globs
    // pass harmlessly to rm (stderr suppressed).
    run("sh", &["-c", &format!("cd '{p1}' && rm -rf -- ..?* .[!.]* * 2>/dev/null; true")])?;
    // Decompress + untar in one pipe (busybox tar's own xz autodetect is unreliable
    // on the box, so drive unxz into tar explicitly).
    run("sh", &["-c", &format!("unxz -c '{recfs}' | tar -x -C '{p1}'")])?;
    run("sync", &[])?;
    let _ = run("umount", &[p1]);
    let _ = run("umount", &[p2]);
    eprintln!("stock: p1 reimaged to stock. Power-cycle to boot Control4.");
    Ok(())
}

/// Reimage p1 by kexec-ing p2's stock recovery kernel with `recovery` on its
/// command line — the same path the recessed button takes, but from software. The
/// recovery kernel's initramfs reformats p1 and unpacks the stock rootfs from p2.
/// p1 is our running root, which is exactly why this must run from a different
/// kernel rather than in place.
///
/// The recovery kernel ships in p2 as recovery_kernel.deb (an ar archive whose
/// data.tar.gz holds the kernel). We extract it with the box's own ar, gunzip and
/// tar, find the bzImage inside (unwrapping the EA3's CEFDK container), and kexec
/// it.
fn recovery_reimage_p1() -> io::Result<()> {
    // A kexec out of a kernel whose SGX driver has bound the GPU leaves the next
    // kernel broken: HPET/IOAPIC reads come back one read late and SDHCI, e1000 and
    // EHCI DMA stall, so the recovery kernel could not reimage p1. Holding the SGX in
    // reset (drivers/gpu/ce5300-sgx-quiesce.c) only saves the case where the GPU
    // never rendered. Refuse rather than strand the box; `stock` gets there by a
    // reboot instead.
    if std::path::Path::new(SGX_DRIVER_LINK).exists() {
        return Err(io::Error::other(
            "the SGX GPU driver is bound: a kexec from this kernel would leave the recovery \
             kernel without working DMA. Use `ohc-restore stock` (one-shot flag + reboot) instead",
        ));
    }
    let p2 = "/mnt/ohc-recovery";
    std::fs::create_dir_all(p2)?;
    run("mount", &["-o", "ro", "/dev/mmcblk0p2", p2])
        .or_else(|_| run("mount", &["-o", "ro,remount", "/dev/mmcblk0p2", p2]))?;
    let deb = format!("{p2}/recovery_kernel.deb");
    if !std::path::Path::new(&deb).exists() {
        return Err(io::Error::other(format!("{deb} missing — cannot recover p1")));
    }
    let work = "/tmp/ohc-rk";
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work)?;
    let tgz = format!("{work}/data.tar.gz");
    let data = std::process::Command::new("ar").args(["p", &deb, "data.tar.gz"]).output()?;
    if !data.status.success() || data.stdout.is_empty() {
        return Err(io::Error::other("ar: could not read data.tar.gz from recovery_kernel.deb"));
    }
    std::fs::write(&tgz, &data.stdout)?;
    // The box's busybox `tar -xa`/`-xz` autodetect does not reliably decompress
    // this member (on the EA3 it silently extracts nothing), so gunzip explicitly
    // first, then untar the plain tarball.
    run("gunzip", &["-f", &tgz])?; // data.tar.gz -> data.tar
    let tar = format!("{work}/data.tar");
    run("tar", &["-xf", &tar, "-C", work])?;

    // Find the kernel. On the EA1 the extracted file is a raw bzImage; on the EA3
    // it is a Control4/CEFDK container (a header — 0x580 bytes on the units seen —
    // then the raw bzImage). find_bzimage locates the setup header and returns
    // where the real bzImage begins; carve it out so kexec gets a clean bzImage.
    let (kern_file, off) = find_bzimage(std::path::Path::new(work))
        .ok_or_else(|| io::Error::other("no bzImage (HdrS magic) found in recovery_kernel.deb"))?;
    let kern = if off == 0 {
        kern_file
    } else {
        eprintln!("stock: unwrapping CEFDK container (bzImage at +0x{off:x})");
        let raw = std::fs::read(&kern_file)?;
        let out = format!("{work}/kernel.bzImage");
        std::fs::write(&out, &raw[off as usize..])?;
        std::path::PathBuf::from(out)
    };
    eprintln!("stock: recovery kernel = {}", kern.display());

    let cmdline = "console=ttyS0,115200 pci=realloc,nocrs,routeirq recovery";
    run("kexec", &["-l", kern.to_str().unwrap(), &format!("--command-line={cmdline}")])?;
    eprintln!("stock: kexec -e (reimaging p1 from p2; the box reboots to stock)");
    run("sync", &[])?;
    run("kexec", &["-e"])?; // does not return
    Ok(())
}

/// Present while a driver (pvrsrvkm) is bound to the CE5300's SGX545, 01:02.0.
const SGX_DRIVER_LINK: &str = "/sys/bus/pci/devices/0000:01:02.0/driver";

/// Walk `dir` and find the Linux bzImage in the recovery payload, returning the
/// file and the byte offset where the real bzImage begins. The setup header's
/// "HdrS" magic is at bzImage+0x202, with the 0x55,0xaa boot signature four bytes
/// before it (bzImage+0x1fe). On the EA1 the extracted file IS a bzImage (offset
/// 0); on the EA3 it is a Control4/CEFDK container and the bzImage starts after a
/// header (0x580 on the units seen). Matching the 0x55,0xaa + "HdrS" pair finds it
/// without assuming either layout.
fn find_bzimage(dir: &std::path::Path) -> Option<(std::path::PathBuf, u64)> {
    // Collect every file, then try kernel-named files FIRST. The 0x55,0xaa+"HdrS"
    // pair can occur by chance in an unrelated binary in the payload (the EA3's
    // recovery tar ships ./tmp/kernel_size_writer, which has such a coincidental
    // match), so a file actually named like a kernel must win over an incidental
    // hit in some tool.
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    let rank = |p: &std::path::Path| -> u8 {
        match p.file_name().and_then(|n| n.to_str()) {
            Some("bzImage") => 0,
            Some(n) if n.contains("bzImage") || n.contains("vmlinuz") || n.contains("vmlinux") => 1,
            _ => 2,
        }
    };
    files.sort_by_key(|p| rank(p));
    for p in files {
        let Ok(buf) = std::fs::read(&p) else { continue };
        if let Some(off) = bzimage_offset(&buf) {
            return Some((p, off));
        }
    }
    None
}

/// Byte offset of the raw bzImage inside `buf`, or None. The setup header "HdrS"
/// sits at bzImage+0x202 with the 0x55,0xaa boot signature at bzImage+0x1fe (four
/// bytes before HdrS); matching that pair locates the kernel whether `buf` is a
/// bare bzImage (offset 0) or a CEFDK container (header, then the kernel). The
/// container header is ~0x580 bytes, so a 64 KiB head bounds the scan.
fn bzimage_offset(buf: &[u8]) -> Option<u64> {
    let limit = buf.len().min(0x10000);
    let mut i = 0x202usize;
    while i + 4 <= limit {
        if &buf[i..i + 4] == b"HdrS" && buf[i - 4] == 0x55 && buf[i - 3] == 0xaa {
            return Some((i - 0x202) as u64);
        }
        i += 1;
    }
    None
}

fn run(cmd: &str, args: &[&str]) -> io::Result<()> {
    let st = std::process::Command::new(cmd).args(args).status()?;
    if !st.success() {
        return Err(io::Error::other(format!("{cmd} {args:?} failed: {st}")));
    }
    Ok(())
}

/// RECOVERY: write the known-good 64 KiB MFH block from a full-flash backup file
/// straight onto the flash, erase + write + READ-BACK-VERIFY. Used to repair a
/// corrupted MFH. `--compensate` writes the source one page (0x100) HIGH, to undo
/// the ce5xx write path's -0x100 address shift (see the controller driver); with
/// it the data lands at 0x80000. Without a verified read-back this never claims
/// success, so it is safe to try both ways.
fn cmd_restore_mfh(file: &str, compensate: bool) -> io::Result<()> {
    let full = std::fs::read(file)?;
    let blk = mfh::TABLE_OFF & !(ERASE_BLOCK - 1); // 0x80000
    if full.len() < blk + ERASE_BLOCK {
        return Err(io::Error::other("backup too small"));
    }
    let src = &full[blk..blk + ERASE_BLOCK]; // the good 64 KiB MFH block
    // sanity: the source really is an MFH (header count in a sane range)
    let cnt = u32::from_le_bytes([src[0x0c], src[0x0d], src[0x0e], src[0x0f]]);
    if !(1..=32).contains(&cnt) {
        return Err(io::Error::other(format!("backup block @0x{blk:x} has no MFH (count={cnt})")));
    }
    println!("restore-mfh: source MFH count={cnt}, compensate={compensate}");

    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(MTD)?;
    let ei = EraseInfoUser { start: blk as u32, length: ERASE_BLOCK as u32 };
    if unsafe { libc::ioctl(f.as_raw_fd(), MEMERASE as _, &ei) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Where to hand the bytes to the (shifting) write path.
    let write_off = if compensate { blk + 0x100 } else { blk } as u64;
    f.seek(SeekFrom::Start(write_off))?;
    f.write_all(src)?;
    f.flush()?;
    drop(f);

    let back = read_block()?; // reads flash @ 0x80000
    if back == src {
        println!("restore-mfh: OK — flash @0x{blk:x} matches the backup, verified.");
        return Ok(());
    }
    // Report the shift so the operator knows which way to compensate.
    let mut shift = None;
    for d in [-0x100i64, 0x100, -0x200, 0x200] {
        let ok = (0..ERASE_BLOCK as i64).all(|i| {
            let j = i + d;
            j < 0 || j >= ERASE_BLOCK as i64 || back[i as usize] == src[j as usize]
        });
        if ok { shift = Some(d); break; }
    }
    Err(io::Error::other(format!(
        "restore-mfh: read-back MISMATCH (apparent shift {:?}); NOT verified — do not reboot",
        shift
    )))
}

/// DIAGNOSTIC: erase the MFH block and report how much actually went to 0xff.
/// If the ce5xx erase works, the whole 64 KiB reads back 0xff. Anything else
/// means the erase did not take (which is why writes can't fix the flash).
fn cmd_erasetest() -> io::Result<()> {
    let blk = (mfh::TABLE_OFF & !(ERASE_BLOCK - 1)) as u32;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(MTD)?;
    let ei = EraseInfoUser { start: blk, length: ERASE_BLOCK as u32 };
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), MEMERASE as _, &ei) };
    println!("erasetest: MEMERASE(0x{blk:x}, 0x{:x}) rc={rc} errno={}",
             ERASE_BLOCK, io::Error::last_os_error());
    drop(f);
    let b = read_block()?;
    let ff = b.iter().filter(|&&x| x == 0xff).count();
    println!("erasetest: after erase, {ff}/{} bytes are 0xff ({}%)",
             b.len(), ff * 100 / b.len());
    println!("erasetest: first 16 bytes: {:02x?}", &b[..16]);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or_default();
    if hc800::is_hc800() {
        let r = match cmd {
            "status" => hc800::status(),
            "stock" => hc800::stock(!args.iter().any(|a| a == "--no-reboot")),
            _ => {
                eprintln!("usage: ohc-restore {{status|stock [--no-reboot]}}   (HC-800)");
                std::process::exit(2);
            }
        };
        if let Err(e) = r {
            eprintln!("ohc-restore {cmd}: {e}");
            std::process::exit(1);
        }
        return;
    }
    let r = match cmd {
        "status" => cmd_status(),
        "erasetest" => cmd_erasetest(),
        "revert" => cmd_revert(),
        "install" => cmd_install(),
        "stock" => cmd_stock(!args.iter().any(|a| a == "--no-reboot")),
        // Reimage p1 + clear the autoscript, in place. boot-init runs this pre-pivot
        // (p1 free) after seeing the one-shot flag; refuses if p1 is the running root.
        "reimage-finalize" => cmd_reimage_finalize(),
        // Reimage p1 by kexec-ing p2's stock recovery kernel. Only from a kernel
        // whose GPU driver never bound (refuses otherwise), and defeated on the EA3
        // by the fat kernel's reclaim memmap; `stock` (flag + reboot) is the path.
        "recovery-kexec" => recovery_reimage_p1(),
        "restore-mfh" => {
            let file = args.get(2).map(String::as_str).unwrap_or("");
            let comp = args.iter().any(|a| a == "--compensate");
            if file.is_empty() {
                eprintln!("usage: ohc-restore restore-mfh <backup.bin> [--compensate]");
                std::process::exit(2);
            }
            cmd_restore_mfh(file, comp)
        }
        _ => {
            eprintln!("usage: ohc-restore {{status|revert|install|stock [--no-reboot]|reimage-finalize|recovery-kexec|restore-mfh <file> [--compensate]}}");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("ohc-restore {cmd}: {e}");
        std::process::exit(1);
    }
}
