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
//!      cannot be done in place; it is handed to CEFDK's own recovery, the same
//!      path the recessed ID button triggers. On the EA that recovery is
//!      CEFDK-driven through p2's recovery scripts, NOT a Linux kexec of a
//!      bzImage (see `recovery_reimage_p1` and packages/restore/DESIGN.md), so
//!      `stock` validates the recovery payload first and refuses rather than
//!      stranding the box when that path is not yet wired up.
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
//!   stock         revert, then kexec p2's recovery kernel to reimage p1. The
//!                 complete, one-way return to stock.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;

use ohc_flash_core::mfh;
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

/// True when a valid MFH item points at the autoscript CONTENT region
/// ([`mfh::EA_CONTENT_OFF`] = 0x91200) — covering BOTH openHC's append_script
/// item (type 0, last) and a CEFDK `mfh add script` item (any slot, type != 0).
/// Keyed on the exact content offset, NOT the whole 0x9xxxx window, so it does
/// not false-match the neighbouring ip_params item at 0x91000. Lets
/// `status`/`install` recognise a CEFDK-created autoscript, not just openHC's.
fn script_item_present(tbl: &[u8]) -> bool {
    let rd = |o: usize| u32::from_le_bytes([tbl[o], tbl[o + 1], tbl[o + 2], tbl[o + 3]]);
    let count = rd(mfh::COUNT_OFF) as usize;
    (0..count).any(|i| {
        let o = mfh::HDR_LEN + i * mfh::ITEM_LEN;
        rd(o) == mfh::FLAG_VALID
            && rd(o + 4) == mfh::EA_CONTENT_OFF
            && rd(o + 0x18) != 0xffff_ffff
    })
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

fn cmd_stock() -> io::Result<()> {
    // Prepare and VALIDATE the p1 reimage BEFORE touching the boot path. If the
    // reimage can't be carried out (the EA3 case today — see
    // `recovery_reimage_p1`), this returns an error and `revert` is never run, so
    // the box is left exactly as it was (on openHC) rather than stranded on
    // "stock kernel item + openHC p1", which boots to nothing. The enable-byte
    // flip is done inside the recovery flow, immediately before the kexec, once a
    // genuinely bootable recovery kernel is in hand.
    recovery_reimage_p1()
}

/// Reimage p1 back to the stock rootfs, from software, the way the recessed ID
/// button does it — without the button.
///
/// p1 is our running root, so it cannot be reformatted in place: the reimage has
/// to run from a *different* kernel. The stock recovery payload ships in p2 as
/// `recovery_kernel.deb` (an `ar` archive whose `data.tar.gz` carries it). We
/// extract it with the box's own `ar` + busybox tooling and inspect it.
///
/// IMPORTANT (measured on the live EA3, 2026-10-15): `./tmp/bzImage` inside that
/// .deb is NOT a Linux bzImage. It is 7,009,216 bytes — byte-for-byte the size of
/// the *stock kernel container* (`cefdk::round`… `0x6af3c0`) — with no `0x55aa`
/// at 0x1fe and a Control4 container header at offset 0, i.e. a `[header][bzImage]`
/// wrapper that only CEFDK knows how to load. `kexec` cannot boot it, and a
/// `-t bzImage64` attempt fails "Not a PE file / can not find .linux section".
/// The .deb also ships `./etc/factory_restore.sh`, `./etc/init.d/init_kernel` and
/// `./tmp/kernel_size_writer`, which is the real shape of the EA recovery: it is
/// CEFDK-driven through those scripts (re-lay the kernel container at eMMC 0x400,
/// set the recovery flag, let CEFDK boot recovery), NOT a plain Linux kexec.
///
/// Until that CEFDK path is implemented and proven, this function extracts and
/// *classifies* the payload, and REFUSES (without reverting the boot path) if it
/// is a container rather than a real bzImage — so `stock` can never half-wipe or
/// strand the box. If a future recovery image genuinely ships a kexec-able
/// bzImage, that path still works: it reverts then kexecs.
fn recovery_reimage_p1() -> io::Result<()> {
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

    // 1. Pull data.tar.gz out of the .deb (ar member) to a file.
    let tgz = format!("{work}/data.tar.gz");
    let data = std::process::Command::new("ar").args(["p", &deb, "data.tar.gz"]).output()?;
    if !data.status.success() || data.stdout.is_empty() {
        return Err(io::Error::other("ar: could not read data.tar.gz from recovery_kernel.deb"));
    }
    std::fs::write(&tgz, &data.stdout)?;

    // 2. Decompress THEN untar. The box's busybox `tar -a` does not autodetect
    //    gzip (the data really is gzip, magic 1f8b), so `tar -xaf data.tar.gz`
    //    dies "invalid tar magic". Do it in two explicit steps: `gzip -dc` (which
    //    busybox does have) to data.tar, then `tar -xf`.
    let tar = format!("{work}/data.tar");
    let dec = std::process::Command::new("gzip").args(["-dc", &tgz]).output()?;
    if !dec.status.success() || dec.stdout.is_empty() {
        return Err(io::Error::other("gzip -dc: could not decompress recovery_kernel.deb data.tar.gz"));
    }
    std::fs::write(&tar, &dec.stdout)?;
    run("tar", &["-xf", &tar, "-C", work])?;

    // 3. Note the recovery scripts the .deb carries — these define the real
    //    (CEFDK-driven) recovery path, and are what the container classification
    //    below points the operator at.
    for s in [
        "etc/factory_restore.sh",
        "etc/init.d/init_kernel",
        "tmp/kernel_size_writer",
    ] {
        if std::path::Path::new(&format!("{work}/{s}")).exists() {
            eprintln!("stock: recovery_kernel.deb ships ./{s}");
        }
    }

    // 4. Classify the payload. A real bzImage we can kexec; a Control4 container
    //    we cannot, and must not pretend to.
    let (kern, payload) = find_kernel_candidate(std::path::Path::new(work))
        .ok_or_else(|| io::Error::other("no kernel-shaped payload found in recovery_kernel.deb"))?;
    let len = std::fs::metadata(&kern).map(|m| m.len()).unwrap_or(0);
    eprintln!("stock: recovery payload = {} ({len} bytes)", kern.display());

    match payload {
        Payload::BzImage => {
            // Genuinely kexec-able: now it is safe to flip the boot path, then go.
            cmd_revert()?;
            let cmdline = "console=ttyS0,115200 pci=realloc,nocrs,routeirq recovery";
            run("kexec", &["-l", kern.to_str().unwrap(), &format!("--command-line={cmdline}")])?;
            eprintln!("stock: kexec -e (reimaging p1 from p2; the box reboots to stock)");
            run("sync", &[])?;
            run("kexec", &["-e"])?; // does not return
            Ok(())
        }
        Payload::Container { inner } => Err(io::Error::other(format!(
            "recovery payload is a Control4 CONTAINER, not a kexec-able bzImage \
             (inner bzImage {}). The EA recovery is CEFDK-driven via the scripts \
             above (factory_restore.sh / init_kernel / kernel_size_writer), not a \
             Linux kexec — see packages/restore/DESIGN.md. Refusing so the box is \
             not stranded on a stock kernel over an openHC p1; boot path left \
             unchanged (still openHC).",
            match inner {
                Some(off) => format!("found at container offset 0x{off:x}"),
                None => "not located".into(),
            }
        ))),
        Payload::Unknown => Err(io::Error::other(
            "recovery payload is neither a bzImage nor a recognised Control4 container; refusing",
        )),
    }
}

/// What a candidate recovery-kernel file actually is.
#[derive(Debug)]
enum Payload {
    /// A raw Linux bzImage (`0x55aa` at 0x1fe, `HdrS` at 0x202) — kexec-able.
    BzImage,
    /// A Control4 container: a wrapper header followed by a bzImage. `inner` is
    /// the byte offset of the wrapped bzImage's start (the `HdrS` position minus
    /// 0x202) when one is found inside, else `None`. NOT kexec-able as-is.
    Container { inner: Option<u64> },
    /// Neither.
    Unknown,
}

/// Classify a file as a bzImage, a Control4 container wrapping one, or neither.
fn classify_payload(data: &[u8]) -> Payload {
    let has_boot_sig = |at: usize| data.get(at..at + 2) == Some(&[0x55, 0xaa][..]);
    let has_hdrs = |at: usize| data.get(at..at + 4) == Some(b"HdrS");
    // Raw bzImage: boot signature at 0x1fe, setup magic at 0x202.
    if has_boot_sig(0x1fe) && has_hdrs(0x202) {
        return Payload::BzImage;
    }
    // Container: find an embedded bzImage by scanning for the HdrS magic and
    // confirming the boot signature sits 4 bytes before it (0x202 -> 0x1fe).
    let mut off = 0usize;
    while off + 4 <= data.len() {
        if has_hdrs(off) && off >= 0x202 && has_boot_sig(off - 0x202 + 0x1fe) {
            return Payload::Container { inner: Some((off - 0x202) as u64) };
        }
        off += 1;
    }
    // A header with no locatable inner bzImage still reads as a container if the
    // boot signature is absent at 0x1fe (a real bzImage would have had it).
    if !has_boot_sig(0x1fe) {
        Payload::Container { inner: None }
    } else {
        Payload::Unknown
    }
}

/// Walk `dir`, classify each file, and return the best kernel candidate: a real
/// bzImage if any file is one, otherwise the first Control4 container found (so
/// the caller can report precisely what the .deb actually holds).
fn find_kernel_candidate(dir: &std::path::Path) -> Option<(std::path::PathBuf, Payload)> {
    let mut stack = vec![dir.to_path_buf()];
    let mut container: Option<(std::path::PathBuf, Payload)> = None;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            match classify_payload(&data) {
                Payload::BzImage => return Some((p, Payload::BzImage)),
                c @ Payload::Container { .. } if container.is_none() => container = Some((p, c)),
                _ => {}
            }
        }
    }
    container
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
        "stock" => cmd_stock(),
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
            eprintln!("usage: ohc-restore {{status|revert|install|stock|restore-mfh <file> [--compensate]}}");
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

    /// A minimal raw bzImage: boot signature at 0x1fe, setup magic at 0x202.
    fn raw_bzimage() -> Vec<u8> {
        let mut v = vec![0u8; 0x1000];
        v[0x1fe] = 0x55;
        v[0x1ff] = 0xaa;
        v[0x202..0x206].copy_from_slice(b"HdrS");
        v
    }

    #[test]
    fn raw_bzimage_is_bzimage() {
        assert!(matches!(classify_payload(&raw_bzimage()), Payload::BzImage));
    }

    #[test]
    fn container_wrapping_a_bzimage_is_container_with_inner_offset() {
        // The EA3 shape: a 0x580-byte Control4 header, then a bzImage. No boot
        // signature at 0x1fe (so it is not mistaken for a raw bzImage), and the
        // inner image's HdrS is at 0x580 + 0x202.
        let header = 0x580usize;
        let mut v = vec![0u8; header];
        v[0] = 0x06; // part of the container header seen on hardware
        v.extend_from_slice(&raw_bzimage());
        match classify_payload(&v) {
            Payload::Container { inner: Some(off) } => assert_eq!(off, header as u64),
            other => panic!("expected Container{{inner: Some(0x{header:x})}}, got {other:?}"),
        }
    }

    #[test]
    fn headerless_blob_without_boot_sig_reads_as_container() {
        // No boot signature at 0x1fe and no embedded HdrS: still clearly not a
        // kexec-able bzImage, so we classify it as a container we can't boot
        // rather than claim it is one.
        let v = vec![0u8; 0x4000];
        assert!(matches!(classify_payload(&v), Payload::Container { inner: None }));
    }

    #[test]
    fn plain_bzimage_boot_sig_but_no_hdrs_is_unknown() {
        let mut v = vec![0u8; 0x1000];
        v[0x1fe] = 0x55;
        v[0x1ff] = 0xaa; // boot sig present, but no HdrS anywhere
        assert!(matches!(classify_payload(&v), Payload::Unknown));
    }
}
