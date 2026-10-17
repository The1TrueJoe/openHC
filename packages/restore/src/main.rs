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
//!   1. REIMAGE p1 from p2    — the stock rootfs (`recfs.tar.xz`). p1 is the
//!      running root, so this cannot be done in place; `stock` kexecs the openHC
//!      kernel into a tiny recovery initramfs (nothing holding p1) that rebuilds
//!      p1 from p2 — Path 2 in packages/restore/DESIGN.md. (The stock
//!      `recovery_kernel.deb` is a kernel *installer*, not a rootfs tool, and its
//!      payload is a Control4 container `kexec` cannot boot — so we use openHC's
//!      own kernel, not CEFDK's recovery.)
//!   2. REMOVE that MFH item  (this tool, over /dev/mtd0) — CEFDK then boots the
//!      stock kernel item again, exactly as from the factory. Done by the recovery
//!      initramfs (`ohc-restore revert`) ONLY after a verified-good reimage, so a
//!      failed extract never strands the box on a stock kernel over a wiped p1.
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
//!   stock --yes   kexec a recovery initramfs that rebuilds p1 from p2, then
//!                 reverts. The complete, one-way return to stock (destructive).

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

/// eMMC byte offset where the openHC kernel bzImage is staged (the durable
/// autoscript's `emmc rd_up` source). `/boot` is empty on the box, so this is the
/// only copy of the kernel available to kexec. We read a generous slice past the
/// image: a bzImage self-describes its length, so kexec ignores the trailing
/// bytes. 8 MiB in (0x800000), confirmed by the "HdrS" magic at 0x800202.
const GAP_KERNEL_OFF_MIB: u64 = 8;
const GAP_KERNEL_READ_MIB: u64 = 12; // ≥ the ~7 MB kernel
const EMMC: &str = "/dev/mmcblk0";
const P2: &str = "/dev/mmcblk0p2";

/// Return the EA to stock Control4 (Path 2 — see packages/restore/DESIGN.md).
///
/// openHC runs *from* p1, so p1 cannot be reimaged in place. We `kexec` the openHC
/// kernel into a tiny purpose-built **recovery initramfs** (nothing holding p1),
/// which rebuilds p1 from p2's stock `recfs.tar.xz`, then runs `ohc-restore revert`
/// (the proven enable-byte + MFH-item clear — the ONLY SPI-NOR write, no new/unvalidated
/// MFH edit) and reboots. On the cold reboot CEFDK boots the stock kernel item over
/// the stock p1: a factory box.
///
/// Fail-safe: the recovery reverts the SPI-NOR autoscript ONLY after a verified
/// good extract. If the extract fails first, the autoscript stays enabled, so the
/// next boot re-runs openHC's `boot-init`, which reinstalls openHC from the
/// eMMC-staged image — the box heals back to openHC rather than stranding.
///
/// Destructive and one-way; guarded by `--yes` (iod / the web UI pass it after
/// their own confirm). NOT yet live-validated — the one destructive on-box test is
/// in DESIGN.md.
fn cmd_stock(yes: bool) -> io::Result<()> {
    if !yes {
        return Err(io::Error::other(
            "`stock` is a destructive factory restore (wipes openHC from p1). \
             Re-run with --yes; iod and the web UI pass it after their own confirm.",
        ));
    }
    // Preconditions, BEFORE anything destructive: the tools the recovery needs,
    // and p2's stock rootfs source.
    for t in ["kexec", "unxz", "tar", "busybox", "cpio", "gzip"] {
        require_tool(t)?;
    }
    let p2mnt = "/mnt/ohc-recovery";
    std::fs::create_dir_all(p2mnt)?;
    let _ = run("mount", &["-o", "ro", P2, p2mnt])
        .or_else(|_| run("mount", &["-o", "ro,remount", P2, p2mnt]));
    let recfs = format!("{p2mnt}/recfs.tar.xz");
    let have_recfs = std::path::Path::new(&recfs).exists();
    let _ = run("umount", &[p2mnt]); // the recovery env mounts p2 itself
    if !have_recfs {
        return Err(io::Error::other(
            "/mnt/ohc-recovery/recfs.tar.xz not found on p2 — no stock rootfs to restore from",
        ));
    }

    // Stage the openHC kernel out of the eMMC gap.
    let work = "/tmp/ohc-stock";
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work)?;
    let kernel = format!("{work}/bzImage");
    dd_extract(EMMC, GAP_KERNEL_OFF_MIB, GAP_KERNEL_READ_MIB, &kernel)?;
    // Sanity: the staged bytes really are a bzImage (HdrS at 0x202).
    {
        let mut f = std::fs::File::open(&kernel)?;
        let mut sig = [0u8; 4];
        f.seek(SeekFrom::Start(0x202))?;
        f.read_exact(&mut sig)?;
        if &sig != b"HdrS" {
            return Err(io::Error::other(
                "staged kernel at eMMC 0x800000 is not a bzImage (no HdrS) — aborting before kexec",
            ));
        }
    }

    // Build the recovery initramfs and kexec into it. No `root=`: the recovery
    // runs entirely from the initramfs, so p1 is free to rebuild.
    let cpio = build_recovery_initramfs(work)?;
    let cmdline = "console=ttyS0,115200 pci=realloc,nocrs,routeirq rw ohc.recovery=stock";
    run("kexec", &["-l", &kernel, &format!("--initrd={cpio}"), &format!("--command-line={cmdline}")])?;
    eprintln!("stock: kexec -e into the RAM recovery (rebuild p1 from p2, revert SPI-NOR, reboot)");
    run("sync", &[])?;
    run("kexec", &["-e"])?; // does not return
    Ok(())
}

/// `dd if=<dev> of=<out> bs=1M skip=<off_mib> count=<read_mib>`.
fn dd_extract(dev: &str, off_mib: u64, read_mib: u64, out: &str) -> io::Result<()> {
    run(
        "dd",
        &[
            &format!("if={dev}"),
            &format!("of={out}"),
            "bs=1M",
            &format!("skip={off_mib}"),
            &format!("count={read_mib}"),
        ],
    )
}

/// True if `tool` is runnable on this image (a real binary or a busybox applet).
fn require_tool(tool: &str) -> io::Result<()> {
    let ok = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {tool} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(format!("required tool '{tool}' not found on this openHC image")))
    }
}

/// The recovery initramfs `/init` (see DESIGN.md Path 2). Kept as a readable shell
/// script — not buried in Rust — so it can be inspected in the cpio and tweaked
/// during the live test. It reuses the existing ext4 on p1 (empties it) rather
/// than re-`mkfs`, so it needs no e2fsprogs library closure, and preserves p1's
/// LABEL/UUID. The SPI-NOR flip is the already-proven `ohc-restore revert`, run
/// ONLY after a verified extract.
const RECOVERY_INIT: &str = r#"#!/bin/sh
# openHC return-to-stock recovery initramfs. kexec'd by `ohc-restore stock`, so
# /dev/mmcblk0p1 is NOT our root and can be rebuilt.
export PATH=/bin:/sbin
mount -t proc     proc     /proc 2>/dev/null
mount -t sysfs    sysfs    /sys  2>/dev/null
mount -t devtmpfs devtmpfs /dev  2>/dev/null
echo
echo "openHC: *** RETURN TO STOCK *** reimaging p1 from p2 recfs.tar.xz"

i=0
while [ ! -b /dev/mmcblk0p1 ] && [ $i -lt 100 ]; do
    mount -t devtmpfs devtmpfs /dev 2>/dev/null
    sleep 0.1 2>/dev/null || usleep 100000 2>/dev/null || :
    i=$((i + 1))
done

mkdir -p /mnt/p1 /mnt/p2
mount -o ro /dev/mmcblk0p2 /mnt/p2 || { echo "FATAL: cannot mount p2"; exec sh; }
[ -f /mnt/p2/recfs.tar.xz ] || { echo "FATAL: no recfs.tar.xz on p2"; exec sh; }

echo "openHC: emptying p1 (keeping its ext4 + label) and extracting stock rootfs..."
umount /mnt/p1 2>/dev/null
mount /dev/mmcblk0p1 /mnt/p1 || { echo "FATAL: cannot mount p1"; exec sh; }
rm -rf /mnt/p1/..?* /mnt/p1/.[!.]* /mnt/p1/* 2>/dev/null
if ! unxz -c /mnt/p2/recfs.tar.xz | tar -x -C /mnt/p1; then
    echo "FATAL: extract failed — leaving the autoscript ENABLED so the box re-boots openHC"
    sync
    exec sh
fi
sync
umount /mnt/p2 2>/dev/null

echo "openHC: stock rootfs in place — reverting the SPI-NOR autoscript (ohc-restore revert)"
# Only now (p1 is good stock) do we disable openHC's boot path, so a failed
# extract above never leaves a box that boots the stock kernel over a wiped p1.
/bin/ohc-restore revert || echo "WARNING: revert failed; clear the autoscript from stock or via serial"
sync
echo "openHC: done — rebooting to stock Control4"
reboot -f
while :; do sleep 10 2>/dev/null || :; done
"#;

/// Assemble the recovery initramfs cpio.gz in `work` and return its path. Mirrors
/// build/mk-boot-init.sh: busybox + its dynamic loader/libc, plus `ohc-restore`
/// itself (for the SPI-NOR revert) and the `/init` above.
fn build_recovery_initramfs(work: &str) -> io::Result<String> {
    let root = format!("{work}/initramfs");
    let _ = std::fs::remove_dir_all(&root);
    for d in ["bin", "sbin", "dev", "proc", "sys", "lib", "mnt/p1", "mnt/p2"] {
        std::fs::create_dir_all(format!("{root}/{d}"))?;
    }
    // busybox + the dynamic loader/libc it needs. On x86_64 the loader lives in
    // /lib64; copy from both and symlink lib64 -> lib so the interp path resolves.
    std::fs::copy("/bin/busybox", format!("{root}/bin/busybox"))?;
    copy_runtime_libs(&format!("{root}/lib"))?;
    let _ = std::os::unix::fs::symlink("lib", format!("{root}/lib64"));

    // ohc-restore itself — the recovery /init calls it for the proven SPI-NOR
    // revert. It is small and (cross-built) often musl-static, so this is cheap.
    let me = std::fs::read_link("/proc/self/exe")
        .unwrap_or_else(|_| std::path::PathBuf::from("/opt/ohc/bin/ohc-restore"));
    std::fs::copy(&me, format!("{root}/bin/ohc-restore"))?;
    let _ = copy_ldd_libs(me.to_str().unwrap_or(""), &format!("{root}/lib"));

    // Applet symlinks busybox resolves by argv[0].
    for a in [
        "sh", "mount", "umount", "mkdir", "rm", "dd", "sync", "cat", "ls", "unxz", "tar", "reboot",
        "sleep", "usleep",
    ] {
        let _ = std::os::unix::fs::symlink("/bin/busybox", format!("{root}/bin/{a}"));
    }

    let init = format!("{root}/init");
    std::fs::write(&init, RECOVERY_INIT)?;
    set_exec(&init)?;

    let out = format!("{work}/recovery.cpio.gz");
    let st = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("cd '{root}' && find . | cpio -o -H newc 2>/dev/null | gzip -9 > '{out}'"),
        ])
        .status()?;
    if !st.success() {
        return Err(io::Error::other("failed to pack the recovery initramfs"));
    }
    Ok(out)
}

/// Copy busybox's likely runtime dependencies (the dynamic loader + libc family)
/// out of /lib and /lib64 into `dst`. Missing ones are harmless; a static busybox
/// simply needs none.
fn copy_runtime_libs(dst: &str) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    let prefixes = [
        "ld-linux", "ld-musl", "ld-", "libc.so", "libm.so", "libresolv", "libdl", "libpthread",
        "librt", "libgcc_s",
    ];
    for dir in ["/lib", "/lib64"] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if prefixes.iter().any(|p| name.starts_with(p)) {
                let _ = std::fs::copy(e.path(), format!("{dst}/{name}"));
            }
        }
    }
    Ok(())
}

/// Copy a binary's resolved shared libraries (from `ldd`) into `dst`, flattening
/// them into the one lib dir. A no-op (Ok) if `ldd` is absent or the binary is
/// static — `copy_runtime_libs` already covers the common case.
fn copy_ldd_libs(bin: &str, dst: &str) -> io::Result<()> {
    let Ok(out) = std::process::Command::new("ldd").arg(bin).output() else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        for tok in line.split_whitespace() {
            if tok.starts_with('/') && tok.contains(".so") {
                if let Some(name) = std::path::Path::new(tok).file_name() {
                    let _ = std::fs::copy(tok, format!("{dst}/{}", name.to_string_lossy()));
                }
            }
        }
    }
    Ok(())
}

fn set_exec(path: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
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
    let f = std::fs::OpenOptions::new().read(true).write(true).open(MTD)?;
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
        "stock" => cmd_stock(args.iter().any(|a| a == "--yes" || a == "--no-prompt")),
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
            eprintln!("usage: ohc-restore {{status|revert|install|stock --yes|restore-mfh <file> [--compensate]}}");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("ohc-restore {cmd}: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The kernel is staged 8 MiB into the eMMC (0x800000), matching the durable
    // autoscript's `emmc rd_up` source and the HdrS magic confirmed at 0x800202.
    #[test]
    fn gap_kernel_offset_is_0x800000() {
        assert_eq!(GAP_KERNEL_OFF_MIB * 1024 * 1024, 0x80_0000);
        assert!(GAP_KERNEL_READ_MIB * 1024 * 1024 >= 0x70_a000, "read must cover the ~7 MB kernel");
    }

    // The recovery /init is Path 2's contract. Pin the invariants a careless edit
    // could break and strand or re-strand a box.
    #[test]
    fn recovery_init_contract() {
        let s = RECOVERY_INIT;
        assert!(s.starts_with("#!/bin/sh"), "must be a shell script");
        // Rebuilds p1 from p2's stock rootfs.
        assert!(s.contains("/mnt/p2/recfs.tar.xz"));
        assert!(s.contains("unxz -c /mnt/p2/recfs.tar.xz | tar -x -C /mnt/p1"));
        // Reuses the existing ext4 (no mkfs) — empties p1 before extracting.
        assert!(s.contains("rm -rf /mnt/p1/..?* /mnt/p1/.[!.]* /mnt/p1/*"));
        assert!(!s.contains("mke2fs") && !s.contains("mkfs"), "Path 2 must not re-mkfs p1");
        // The ONLY SPI-NOR write is the proven revert, and it must come AFTER the
        // extract (the extract's failure path must be above the revert).
        let revert = s.find("ohc-restore revert").expect("must revert the SPI-NOR autoscript");
        let extract_fail = s.find("FATAL: extract failed").expect("extract must have a failure guard");
        assert!(extract_fail < revert, "revert must only run after a successful extract");
        assert!(s.contains("reboot -f"));
    }
}
