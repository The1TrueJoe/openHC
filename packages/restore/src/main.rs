//! ohc-restore — return a Control4 EA controller to stock.
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
//!      cannot be done in place; it is handed to CEFDK's own recovery kernel,
//!      which is what the recessed button triggers and what `stock` kexecs.
//!
//! The MFH is SHA-256 protected and a bad write bricks past the button (recovery
//! needs an external programmer), so every write here is read-modify-ERASE-write
//! then READ-BACK-VERIFIED, and the table edit is the byte-for-byte inverse of
//! the install's (see mfh.rs, shared with the flasher and unit-tested).
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

mod mfh;

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

fn read_block() -> io::Result<Vec<u8>> {
    let mut f = std::fs::OpenOptions::new().read(true).open(MTD)?;
    f.seek(SeekFrom::Start(BLOCK_OFF))?;
    let mut buf = vec![0u8; ERASE_BLOCK];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Erase the MFH block and write `block` (the full 64 KiB) back, then read it
/// back and confirm it is identical. Anything but an exact match is an error —
/// we never leave the box with a half-written MFH silently.
fn erase_and_write(block: &[u8]) -> io::Result<()> {
    assert_eq!(block.len(), ERASE_BLOCK);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(MTD)?;
    let ei = EraseInfoUser { start: BLOCK_OFF as u32, length: ERASE_BLOCK as u32 };
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), MEMERASE as _, &ei) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    f.seek(SeekFrom::Start(BLOCK_OFF))?;
    f.write_all(block)?;
    f.flush()?;
    drop(f);

    let back = read_block()?;
    if back != block {
        return Err(io::Error::other("read-back verify FAILED — MFH block mismatch"));
    }
    Ok(())
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
    let openhc = count >= 2
        && rd(last + 0x18) == mfh::TYPE_SCRIPT
        && (0x9_0000..0xa_0000).contains(&rd(last + 4));
    println!(
        "state: {}",
        if openhc { "openHC (last item is the install's script)" } else { "stock (no openHC script item)" }
    );
    Ok(())
}

fn cmd_revert() -> io::Result<()> {
    let mut block = read_block()?;
    let removed = mfh::remove_script(table_slice(&mut block))
        .map_err(io::Error::other)?;
    println!("revert: MFH now has {removed} items (stock); writing + verifying...");
    erase_and_write(&block)?;
    println!("revert: OK — CEFDK will boot the stock kernel item. (p1 still openHC;");
    println!("        run `ohc-restore install` to undo, or `stock` to finish.)");
    Ok(())
}

fn cmd_install() -> io::Result<()> {
    let mut block = read_block()?;
    mfh::append_script(table_slice(&mut block), mfh::EA_CONTENT_OFF, SCRIPT_LEN, mfh::TYPE_KERNEL)
        .map_err(io::Error::other)?;
    println!("install: re-appended openHC's MFH script item; writing + verifying...");
    erase_and_write(&block)?;
    println!("install: OK — CEFDK will boot openHC again.");
    Ok(())
}

fn cmd_stock() -> io::Result<()> {
    cmd_revert()?;
    println!("stock: MFH reverted. Handing p1 to CEFDK's recovery kernel...");
    recovery_reimage_p1()?;
    Ok(())
}

/// Reimage p1 by kexec-ing p2's stock recovery kernel with `recovery` on its
/// command line — the same path the recessed button takes, but from software. The
/// recovery kernel's initramfs reformats p1 and unpacks the stock rootfs from p2.
/// p1 is our running root, which is exactly why this must run from a different
/// kernel rather than in place.
///
/// The recovery kernel ships in p2 as recovery_kernel.deb (an ar archive whose
/// data.tar.gz holds the kernel). We extract it with the box's own ar + busybox
/// tar (`-xa`, autodetect), find the bzImage inside, and kexec it.
fn recovery_reimage_p1() -> io::Result<()> {
    let p2 = "/mnt/ohc-recovery";
    std::fs::create_dir_all(p2)?;
    run("mount", &["-o", "ro", "/dev/mmcblk0p2", p2])
        .or_else(|_| run("mount", &["-o", "ro,remount", "/dev/mmcblk0p2", p2]))?;
    let deb = format!("{p2}/recovery_kernel.deb");
    if !std::path::Path::new(&deb).exists() {
        return Err(io::Error::other(format!("{deb} missing — cannot recover p1")));
    }
    // ar p <deb> data.tar.gz | tar -x -a -f - -C /tmp/ohc-rk
    let work = "/tmp/ohc-rk";
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work)?;
    let tgz = format!("{work}/data.tar.gz");
    let data = std::process::Command::new("ar").args(["p", &deb, "data.tar.gz"]).output()?;
    if !data.status.success() || data.stdout.is_empty() {
        return Err(io::Error::other("ar: could not read data.tar.gz from recovery_kernel.deb"));
    }
    std::fs::write(&tgz, &data.stdout)?;
    run("tar", &["-x", "-a", "-f", &tgz, "-C", work])?;

    // Find the kernel: a file whose 0x202 magic is "HdrS" (the bzImage setup sig).
    let kern = find_bzimage(std::path::Path::new(work))
        .ok_or_else(|| io::Error::other("no bzImage (HdrS magic) found in recovery_kernel.deb"))?;
    eprintln!("stock: recovery kernel = {}", kern.display());

    let cmdline = "console=ttyS0,115200 pci=realloc,nocrs,routeirq recovery";
    run("kexec", &["-l", kern.to_str().unwrap(), &format!("--command-line={cmdline}")])?;
    eprintln!("stock: kexec -e (reimaging p1 from p2; the box reboots to stock)");
    run("sync", &[])?;
    run("kexec", &["-e"])?; // does not return
    Ok(())
}

/// Walk `dir` and return the first file that is a Linux bzImage (the "HdrS" setup
/// magic at offset 0x202), so we kexec the kernel regardless of how the .deb lays
/// its payload out.
fn find_bzimage(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if let Ok(mut f) = std::fs::File::open(&p) {
                let mut sig = [0u8; 4];
                use std::io::{Read, Seek, SeekFrom};
                if f.seek(SeekFrom::Start(0x202)).is_ok() && f.read_exact(&mut sig).is_ok() && &sig == b"HdrS" {
                    return Some(p);
                }
            }
        }
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

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    let r = match cmd.as_str() {
        "status" => cmd_status(),
        "revert" => cmd_revert(),
        "install" => cmd_install(),
        "stock" => cmd_stock(),
        _ => {
            eprintln!("usage: ohc-restore {{status|revert|install|stock}}");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("ohc-restore {cmd}: {e}");
        std::process::exit(1);
    }
}
