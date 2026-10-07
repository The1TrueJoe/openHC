//! HC-800 return-to-stock, run on the box itself.
//!
//! The HC-800 boots GRUB 0.97 off a plain-text `menu.lst` on sda1, and the
//! factory restore is Control4's own: entry 0 boots the untouched restore system
//! on sda2, which re-images sda3/sda4 from its recorded tarballs. So "stock" is
//! a one-shot GRUB entry and a reboot — the same thing the flasher's
//! `ohc-flash restore` does over SSH, using the SAME menu rewrite
//! ([`hc::factory_once_menu`], from the flasher's core crate and unit-tested
//! there), so the web UI and the flasher cannot drift apart.
//!
//! The rules this board lives by (sda1 is the one unrecoverable failure):
//!   * sda2, the MBR and the `support_factorydefault`/`factorydefault` lines are
//!     never written — `factory_once_menu` refuses a menu without them;
//!   * menu.lst is backed up beside itself before it is written, and read back
//!     after; a mismatch puts the backup back;
//!   * the `default` file is written and read back BEFORE menu.lst says
//!     `default saved` — GRUB with `default saved` and no file falls back to
//!     entry 0, a restore that runs forever.

use ohc_flash_core::hc800 as hc;
use std::io;
use std::process::Command;

const MNT: &str = "/mnt/ohc-grub";

/// sda1 mounted at [`MNT`] for as long as this lives; synced and unmounted on
/// drop, so no early return can leave the bootloader partition mounted.
struct Grub;

impl Grub {
    fn mount(rw: bool) -> io::Result<Grub> {
        std::fs::create_dir_all(MNT)?;
        let mode = if rw { "rw" } else { "ro" };
        let mounted = std::fs::read_to_string("/proc/mounts")
            .map(|m| m.lines().any(|l| l.split_whitespace().nth(1) == Some(MNT)))
            .unwrap_or(false);
        if mounted {
            run("mount", &["-o", &format!("remount,{mode}"), MNT])?;
        } else {
            run("mount", &["-o", mode, hc::GRUB_PART, MNT])?;
        }
        Ok(Grub)
    }
    fn menu(&self) -> String {
        format!("{MNT}/boot/grub/menu.lst")
    }
    fn default(&self) -> String {
        format!("{MNT}/boot/grub/default")
    }
}

impl Drop for Grub {
    fn drop(&mut self) {
        let _ = run("sync", &[]);
        let _ = run("umount", &[MNT]);
    }
}

/// What the boot chain says right now. The `state:` line is what iod keys on
/// (`state: openHC` = there is an openHC install to restore from).
pub fn status() -> io::Result<()> {
    let g = Grub::mount(false)?;
    let menu = std::fs::read_to_string(g.menu())?;
    let default = menu
        .lines()
        .find(|l| l.trim_start().starts_with("default"))
        .map(|l| l.split_whitespace().nth(1).unwrap_or("?").to_string())
        .unwrap_or_else(|| "?".into());
    println!("GRUB {} menu.lst: default {default}", hc::GRUB_PART);
    let titles = hc::titles(&menu);
    for (i, t) in titles.iter().enumerate() {
        println!("  [{i}] {t}");
    }
    let state = if titles.iter().any(|t| t == hc::FACTORY_ONCE_TITLE) {
        "restore pending (the next boot runs Control4's factory restore once)"
    } else if titles.iter().any(|t| t.starts_with("openHC")) {
        "openHC (installed; restorable to stock)"
    } else {
        "stock (no openHC entry)"
    };
    println!("state: {state}");
    Ok(())
}

/// Arm Control4's factory restore for exactly one boot, then reboot into it.
/// The restore wipes sda3/sda4 (openHC's kernel goes with them) and its own
/// closing reboot lands on the freshly restored stock image.
///
/// `reboot: false` (`stock --no-reboot`) stops after the verified write, for
/// the flasher: it reads the result over SSH, then reboots the box itself so
/// the connection is not torn down mid-reply.
pub fn stock(reboot: bool) -> io::Result<()> {
    let g = Grub::mount(true)?;
    let (menu, dflt) = (g.menu(), g.default());
    let before = std::fs::read_to_string(&menu)?;
    let (out, once) = hc::factory_once_menu(&before).map_err(io::Error::other)?;

    let backup = format!("{menu}.pre-factory-restore.{}", now());
    std::fs::copy(&menu, &backup)?;
    println!("stock: kept {backup}");

    // 1. The default file -> the one-shot entry, read back before menu.lst
    //    is allowed to depend on it.
    let text = match std::fs::read_to_string(&dflt) {
        Ok(t) if !t.is_empty() => {
            let rest = t.split_once('\n').map(|(_, r)| r).unwrap_or("");
            format!("{once}\n{rest}")
        }
        _ => hc::default_file(once),
    };
    std::fs::write(&dflt, &text)?;
    run("sync", &[])?;
    let first = std::fs::read_to_string(&dflt)?.lines().next().unwrap_or("").trim().to_string();
    if first != once.to_string() {
        return Err(io::Error::other(format!(
            "{dflt} reads back {first:?}, expected {once} — menu.lst NOT changed"
        )));
    }
    println!("stock: saved default -> entry {once} ({})", hc::FACTORY_ONCE_TITLE);

    // 2. menu.lst, read back before trusting it.
    std::fs::write(&menu, &out)?;
    run("sync", &[])?;
    if std::fs::read_to_string(&menu)? != out {
        std::fs::copy(&backup, &menu)?;
        return Err(io::Error::other("menu.lst read back differently than written — restored the backup"));
    }
    println!(
        "stock: menu.lst verified ({} bytes): entry {once} restores once, then entry {} boots",
        out.len(),
        hc::ENTRY_VENDOR
    );
    drop(g);

    if !reboot {
        println!("stock: armed; the next boot runs Control4's factory restore");
        return Ok(());
    }
    println!("stock: rebooting into Control4's factory restore (~5 min, then stock Control4)");
    run("reboot", &[])
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run(cmd: &str, args: &[&str]) -> io::Result<()> {
    let st = Command::new(cmd).args(args).status()?;
    if !st.success() {
        return Err(io::Error::other(format!("{cmd} {args:?} failed: {st}")));
    }
    Ok(())
}

/// True when this rootfs is an HC-800 image (`board=hc800` in the release file
/// the post-build hook writes).
pub fn is_hc800() -> bool {
    std::fs::read_to_string("/etc/openhc-release")
        .map(|r| r.lines().any(|l| l.trim() == "board=hc800"))
        .unwrap_or(false)
}
