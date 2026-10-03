//! HC-800 install flows: `kexec` (writes nothing) and `grub` (persistent).
//!
//! Both are short, because this board hands you a working, unlocked PC boot
//! chain and asks nothing in return: no container format, no secure-boot fuse,
//! no autoscript in SPI-NOR. The care here is spent almost entirely on **not
//! breaking the way back**, which on this board means `/dev/sda1`.

use anyhow::{bail, Context, Result};
use ohc_flash_core::hc800 as hc;
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};
use crate::release::Release;

/// Where images are staged on the box: a tmpfs of our own, sized to the
/// release, on whichever OS is running. RAM only, so a kexec launch still writes
/// no disk.
///
/// Not `/tmp`: the stock image's is a 32 MB tmpfs, and an hc800 release grew
/// past that (13.8 MB kernel + 29.7 MB initramfs once zigbee2mqtt's Node.js
/// runtime landed), at which point the old staging failed with a short write.
const STAGE: &str = "/mnt/ohc-stage";

/// Headroom on top of the images when sizing [`STAGE`]: the pushed kexec
/// (~1.5 MB) plus slack.
const STAGE_SLACK_MB: usize = 16;

/// Pull the kernel and initramfs out of a release, accepting either the raw
/// Buildroot names or the bundle's prefixed ones — the CI zip carries both.
fn images(rel: &Release) -> Result<(&[u8], &[u8])> {
    let kernel = rel
        .get("openhc-hc800-kernel.img")
        .or_else(|| rel.get("bzImage"))
        .context("release has no bzImage")?;
    let initrd = rel
        .get("openhc-initrd.gz")
        .or_else(|| rel.get("rootfs.cpio.gz"))
        .context("release has no rootfs.cpio.gz")?;
    Ok((kernel, initrd))
}

/// Push both images to `$STAGE`, returning their remote paths.
fn stage(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<(String, String)> {
    let (kernel, initrd) = images(rel)?;
    let mb = (kernel.len() + initrd.len()) / 1_048_576 + 1 + STAGE_SLACK_MB;
    // A stale stage from an earlier run is unmounted first, so its size and
    // contents are never what this run relies on.
    ssh.run(
        &format!(
            "umount {STAGE} 2>/dev/null; mkdir -p {STAGE} && mount -t tmpfs -o size={mb}m ohc-stage {STAGE}"
        ),
        true,
    )
    .map_err(|e| anyhow::anyhow!("cannot mount a {mb} MB tmpfs at {STAGE}: {e}"))?;
    let kp = format!("{STAGE}/openhc-bzImage");
    let ip = format!("{STAGE}/openhc-initrd.gz");
    p.emit(Event::step(format!(
        "staging {} MB to {STAGE}",
        (kernel.len() + initrd.len()) / 1_048_576
    )));
    ssh.put_stream(kernel, &format!("cat > {kp}")).map_err(|e| anyhow::anyhow!("{e}"))?;
    ssh.put_stream(initrd, &format!("cat > {ip}")).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Verify the length landed. A short write here becomes "the kernel loads
    // and then the console fills with garbage", which is a much worse place to
    // discover a truncated transfer than right now.
    for (path, want) in [(&kp, kernel.len()), (&ip, initrd.len())] {
        let got: usize = ssh
            .run(&format!("wc -c < {path}"), true)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .trim()
            .parse()
            .unwrap_or(0);
        if got != want {
            bail!("{path}: staged {got} bytes, expected {want}");
        }
        p.emit(Event::detail(format!("{path}: {got} bytes")));
    }
    Ok((kp, ip))
}

/// Build a `netconsole=` boot argument aimed at `(ip, port)`.
///
/// THE DESTINATION MAC IS RESOLVED ON THE BOX, not guessed here. netpoll writes
/// the Ethernet header itself — no ARP, no routing, which is exactly what lets
/// it keep logging from inside a panic — so it needs a real MAC, and the only
/// machine that can see the right one is the box.
///
/// Broadcast is NOT a usable fallback, which is worth stating because it looks
/// like one: `ff:ff:ff:ff:ff:ff` was tried against a listener on the same flat
/// segment and delivered **nothing at all**. So a failed resolve returns None
/// and the caller boots without netconsole rather than with a target that
/// silently goes nowhere.
fn netconsole_arg(ssh: &Ssh, ip: &str, port: u16, uplink: &str) -> Option<String> {
    // Ping first so the entry is fresh, then read the box's own ARP table.
    let cmd = format!(
        r#"ping -c 1 -W 1 {ip} >/dev/null 2>&1; awk '$1 == "{ip}" && $4 != "00:00:00:00:00:00" {{ print $4 }}' /proc/net/arp"#
    );
    let mac = ssh.run(&cmd, false).ok()?.split_whitespace().next()?.to_string();
    if mac.len() != 17 {
        return None;
    }

    // THE SOURCE ADDRESS HAS TO BE LITERAL. netconsole is set up from the boot
    // line at about four seconds, well before DHCP has finished, so leaving the
    // source empty makes netpoll try to read it off the interface and give up:
    //
    //   netpoll: netconsole: no IP address for eth0, aborting
    //   netconsole: Not enabling netconsole for cmdline0. Netpoll setup failed
    //
    // which is silent from the operator's side — the listener simply never sees
    // a packet, exactly as if the box had died. So we use the address the box
    // has RIGHT NOW, before the kexec. It is only a UDP source address; nothing
    // needs it to still be true afterwards, and the lease is usually the same
    // one anyway.
    let src = ssh
        .run(&format!("ip -4 addr show {uplink} 2>/dev/null | awk '$1 == \"inet\" {{ print $2 }}'"), false)
        .ok()?;
    let src = src.split_whitespace().next()?.split('/').next()?.to_string();
    if src.is_empty() {
        return None;
    }
    Some(format!("netconsole=6665@{src}/{uplink},{port}@{ip}/{mac}"))
}

/// Start openHC out of the running system. **Writes to no partition.**
///
/// `netconsole` is `(listener ip, port)`; worth passing whenever the caller
/// knows where to listen, because the window between `kexec -e` and the new
/// kernel bringing up the NIC is the only part of this that a serial cable can
/// see and SSH cannot.
pub fn kexec(ssh: &Ssh, rel: &Release, netconsole: Option<(&str, u16)>, p: &Progress) -> Result<()> {
    let (kp, ip) = stage(ssh, rel, p)?;

    // The vendor image's kernel is CONFIG_KEXEC=y but Control4 never shipped
    // the userspace tool, so a stock box needs one pushed. openHC has its own.
    // The hc800 release bundles a static i686 one (images.yml), pushed here.
    let kexec_bin = if ssh.run("command -v kexec", false).map(|s| !s.trim().is_empty()).unwrap_or(false)
    {
        "kexec".to_string()
    } else if let Some(bin) = rel.get("kexec-i686-static") {
        let staged = format!("{STAGE}/kexec");
        ssh.put_stream(bin, &format!("cat > {staged} && chmod +x {staged}"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        p.emit(Event::detail(format!("pushed the release's static i686 kexec ({} KB)", bin.len() / 1024)));
        staged
    } else {
        let _ = ssh.run(&format!("umount {STAGE}"), false);
        bail!(
            "no kexec on this system, and this release does not carry kexec-i686-static. \
             The stock Control4 image ships none; use an hc800 release built after the \
             bundle started including it, or the artifact from .github/workflows/tools.yml"
        );
    };

    // Stop our own watchdog before the handover. kexec -e runs no shutdown
    // script, so a kicker left running would be killed mid-jump WITHOUT the
    // magic close — which is precisely how you arm the timer rather than
    // disarm it. Harmless on the stock image, which has no such script.
    let _ = ssh.run("/etc/init.d/S02watchdog stop 2>/dev/null; true", false);

    let mut append = hc::CMDLINE.to_string();
    if let Some((ip, port)) = netconsole {
        // The uplink's name as the BOX sees it, from its own board.env, rather
        // than an assumed eth0 — a board with a managed switch calls it
        // something else and the argument would point at a device that is not
        // there.
        let uplink = ssh
            .read_file("/opt/ohc/board.env")
            .and_then(|e| {
                e.lines()
                    .find_map(|l| l.trim().strip_prefix("OHC_UPLINK_IFACE=").map(str::to_string))
            })
            .map(|v| v.split('#').next().unwrap_or("").trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "eth0".into());
        match netconsole_arg(ssh, ip, port, &uplink) {
            Some(nc) => {
                p.emit(Event::detail(format!("netconsole: {nc}")));
                append.push(' ');
                append.push_str(&nc);
            }
            None => p.emit(Event::warn(format!(
                "could not resolve {ip}'s MAC from the box — booting without netconsole,                  because a target with a guessed MAC delivers nothing and looks like a dead box"
            ))),
        }
    }

    p.emit(Event::step("kexec -l (staging into the running kernel)".into()));
    if let Err(e) = ssh.run(&format!("{kexec_bin} -l {kp} --initrd={ip} --append='{append}'"), true) {
        let _ = ssh.run(&format!("umount {STAGE}"), false);
        bail!("kexec -l refused the image: {e}");
    }

    p.emit(Event::step("kexec -e — this connection will drop".into()));
    // Detached, a second after this session returns. Run in the foreground,
    // kexec -e never closes the TCP connection — the kernel underneath it is
    // simply gone — so ssh waited on a dead socket indefinitely (seen on the
    // unit: openHC at a login prompt, the flasher still "running").
    let _ = ssh.run(
        &format!("sync; nohup sh -c 'sleep 1; {kexec_bin} -e' >/dev/null 2>&1 </dev/null &"),
        false,
    );
    p.emit(Event::resolved(
        "openHC is starting. It takes a fresh DHCP lease, so find it by MAC, not by its old address"
            .into(),
    ));
    Ok(())
}

/// The persistent install. This is the only flow in the tool that writes the
/// bootloader partition, so it checks before it does and reads back after.
pub fn install_grub(ssh: &Ssh, rel: &Release, boot_once: bool, p: &Progress) -> Result<()> {
    let (kernel, initrd) = images(rel)?;
    let gm = "/mnt/ohc-grub";
    let km = "/mnt/ohc-kernel";

    // --- the kernel partition: files only, nothing raw ----------------------
    p.emit(Event::step(format!("mounting {} ({})", hc::KERNEL_PART, hc::KERNEL_LABEL)));
    ssh.run(&format!("mkdir -p {km} && mount {} {km}", hc::KERNEL_PART), true)
        .map_err(|e| anyhow::anyhow!("cannot mount {}: {e}", hc::KERNEL_PART))?;

    let free: u64 = ssh
        .run(&format!("df -k {km} | awk 'NR==2 {{print $4}}'"), true)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .trim()
        .parse()
        .unwrap_or(0);
    let need = ((kernel.len() + initrd.len()) / 1024) as u64;
    if free < need + 4096 {
        let _ = ssh.run(&format!("umount {km}"), false);
        bail!("{} has {free} KB free, need {need} KB plus headroom", hc::KERNEL_PART);
    }
    p.emit(Event::detail(format!("{free} KB free, writing {need} KB")));

    ssh.put_stream(kernel, &format!("cat > {km}{}", hc::KERNEL_FILE))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    ssh.put_stream(initrd, &format!("cat > {km}{}", hc::INITRD_FILE))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for (f, want) in [(hc::KERNEL_FILE, kernel.len()), (hc::INITRD_FILE, initrd.len())] {
        let got: usize = ssh
            .run(&format!("wc -c < {km}{f}"), true)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .trim()
            .parse()
            .unwrap_or(0);
        if got != want {
            let _ = ssh.run(&format!("umount {km}"), false);
            bail!("{f}: wrote {got} bytes, expected {want}");
        }
        p.emit(Event::detail(format!("{f}: {got} bytes")));
    }
    ssh.run(&format!("sync; umount {km}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;

    // --- the bootloader partition: the part worth being careful about -------
    p.emit(Event::step(format!("mounting {} ({})", hc::GRUB_PART, hc::GRUB_LABEL)));
    ssh.run(&format!("mkdir -p {gm} && mount {} {gm}", hc::GRUB_PART), true)
        .map_err(|e| anyhow::anyhow!("cannot mount {}: {e}", hc::GRUB_PART))?;

    let menu = format!("{gm}/boot/grub/menu.lst");
    let on_disk = ssh
        .read_file(&menu)
        .with_context(|| format!("{menu} is unreadable — refusing to write a bootloader blind"))?;
    // A spent factory-restore-once entry from `restore` would otherwise sit at
    // ENTRY_OPENHC and push our entry past it. See hc::drop_entry.
    let no_once = hc::drop_entry(&on_disk, hc::FACTORY_ONCE_TITLE);
    if no_once != on_disk {
        p.emit(Event::detail(format!("dropped the spent `{}` entry", hc::FACTORY_ONCE_TITLE)));
    }
    // An existing openHC entry is rebuilt, not kept: the mode lives in it (a
    // boot-once entry carries `savedefault`), so keeping it meant a reinstall
    // could never switch between boot-once and persistent.
    let before = hc::drop_entry(&no_once, "openHC");
    if before != no_once {
        p.emit(Event::detail("replacing the existing openHC entry".into()));
    }

    // The two lines Control4 patched in are what the hardware factory-default
    // button reads. If they are not where we expect, this is not the menu.lst
    // this tool was written against and it should not be editing it.
    for line in hc::GUARDED_LINES {
        if !before.lines().any(|l| l.trim_start().starts_with(line)) {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!("menu.lst has no `{line}` line — the factory-default button depends on it");
        }
    }
    // One backup on the box itself, alongside the one that should already be on
    // the operator's machine. Cheap, and the file is ~1 KB. Never overwritten:
    // the FIRST one is the as-shipped file, which is what uninstall should put
    // back — a later copy would be whatever an earlier restore or install left.
    ssh.run(&format!("[ -s {menu}.pre-openhc ] || cp {menu} {menu}.pre-openhc"), true)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    p.emit(Event::detail(format!("{menu}.pre-openhc in place")));

    // The `default` line, which is the whole difference between the two modes:
    //
    //   boot-once  -> `default saved`, and the entry's `savedefault 1` hands it
    //                 back to Control4 on every openHC boot.
    //   persistent -> `default 2`, and openHC is simply what this box runs.
    //
    // Only that one line changes; the two vendor entries are appended past and
    // never rewritten.
    let want = if boot_once {
        "default\t\tsaved".to_string()
    } else {
        format!("default\t\t{}", hc::ENTRY_OPENHC)
    };
    let mut out = String::new();
    let mut seen_default = false;
    for l in before.lines() {
        if l.trim_start().starts_with("default") {
            seen_default = true;
            out.push_str(&want);
            out.push('\n');
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !seen_default {
        let _ = ssh.run(&format!("umount {gm}"), false);
        bail!("menu.lst has no `default` line at all — not the file this tool expects");
    }
    out.push_str(&hc::menu_entry(boot_once));
    // `default` is a NUMBER: refuse to write a file where it names anything
    // but our entry (with a stray extra vendor entry it would be a factory
    // restore or a stock boot instead).
    let titles = hc::titles(&out);
    if titles.get(hc::ENTRY_OPENHC as usize).map(String::as_str) != Some("openHC") {
        let _ = ssh.run(&format!("umount {gm}"), false);
        bail!("openHC would not be entry {} ({titles:?}) — not writing menu.lst", hc::ENTRY_OPENHC);
    }

    ssh.put_stream(out.as_bytes(), &format!("cat > {menu}")).map_err(|e| anyhow::anyhow!("{e}"))?;
    if boot_once {
        ssh.put_stream(
            hc::default_file(hc::ENTRY_OPENHC).as_bytes(),
            &format!("cat > {gm}/boot/grub/default"),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    } else {
        // Only `default saved` reads this file, and we have just written
        // `default N`. Leaving one behind — from an earlier --boot-once install,
        // say — is a file that names an entry nothing consults, which is exactly
        // the sort of thing someone later reads as the truth.
        let _ = ssh.run(&format!("rm -f {gm}/boot/grub/default"), false);
    }

    // Read back. A bootloader partition is the one place where "the write
    // returned success" is not good enough.
    let after = ssh.read_file(&menu).context("menu.lst unreadable after writing it")?;
    if after != out {
        let _ = ssh.run(&format!("cp {menu}.pre-openhc {menu}; sync; umount {gm}"), false);
        bail!("menu.lst read back differently than written — restored the backup, nothing changed");
    }
    for line in hc::GUARDED_LINES {
        if !after.lines().any(|l| l.trim_start().starts_with(line)) {
            let _ = ssh.run(&format!("cp {menu}.pre-openhc {menu}; sync; umount {gm}"), false);
            bail!("`{line}` did not survive the write — restored the backup");
        }
    }
    p.emit(Event::detail(format!("menu.lst verified, {} bytes", after.len())));
    ssh.run(&format!("sync; umount {gm}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;

    p.emit(Event::resolved(if boot_once {
        format!(
            "installed as a BOOT-ONCE. The next boot runs openHC; every openHC boot then \
             re-points the default at entry {} (Control4), so a reset of any kind returns to stock",
            hc::ENTRY_VENDOR
        )
    } else {
        format!(
            "installed as the DEFAULT. Every boot runs openHC, including after a power cut. \
             `fallback {}` still catches a kernel that will not load; a kernel that loads and \
             then panics will reboot into itself, and the ID button held at power-on is the way \
             out of that",
            hc::ENTRY_VENDOR
        )
    }));
    Ok(())
}

/// Point the saved GRUB default at an installed openHC and reboot into it.
///
/// This is the other half of [`install_grub`], and it exists because the
/// boot-once property deliberately makes openHC forget itself: every openHC
/// boot runs `savedefault` and hands the default straight back to Control4. So
/// after any reset the box is on stock with openHC still sitting on disk, and
/// getting back is not a reinstall — it is one byte, changed here.
///
/// Refuses if there is no openHC entry, rather than setting a default that
/// points at nothing.
pub fn boot_installed(ssh: &Ssh, p: &Progress) -> Result<()> {
    let gm = "/mnt/ohc-grub";
    ssh.run(&format!("mkdir -p {gm} && mount {} {gm}", hc::GRUB_PART), true)
        .map_err(|e| anyhow::anyhow!("cannot mount {}: {e}", hc::GRUB_PART))?;

    let menu = ssh.read_file(&format!("{gm}/boot/grub/menu.lst")).unwrap_or_default();
    let titles = hc::titles(&menu);
    match titles.iter().position(|t| t == "openHC") {
        None => {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!("no openHC entry in menu.lst — install it first (--method grub)");
        }
        // The saved default below is a NUMBER. If the entry is not where that
        // number points, this would boot something else — a factory restore,
        // for one.
        Some(i) if i != hc::ENTRY_OPENHC as usize => {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!(
                "the openHC entry is entry {i}, not {} ({titles:?}) — reinstall (--method grub) to fix the order",
                hc::ENTRY_OPENHC
            );
        }
        Some(_) => {}
    }
    for f in [hc::KERNEL_FILE, hc::INITRD_FILE] {
        let km = "/mnt/ohc-kernel";
        let present = ssh
            .run(
                &format!("mkdir -p {km} && mount -o ro {} {km} && test -s {km}{f} && echo yes; umount {km} 2>/dev/null", hc::KERNEL_PART),
                false,
            )
            .map(|o| o.contains("yes"))
            .unwrap_or(false);
        if !present {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!("menu.lst names {f} but it is not on {} — refusing to boot a missing kernel", hc::KERNEL_PART);
        }
    }

    // Only the first line changes. The rest of the file is padding that GRUB's
    // `savedefault` rewrites in place, by sector — replacing the whole file
    // would move its blocks and quietly break that.
    ssh.run(
        &format!(
            "sed -i '1s/.*/{}/' {gm}/boot/grub/default && sync && umount {gm}",
            hc::ENTRY_OPENHC
        ),
        true,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    p.emit(Event::step(format!("saved default -> entry {} (openHC)", hc::ENTRY_OPENHC)));

    p.emit(Event::step("rebooting — this connection will drop".into()));
    let _ = ssh.run("sync; reboot", false);
    p.emit(Event::resolved(
        "on its way. openHC will hand the default straight back to Control4 as it boots, so this \
         is a one-shot: any later reset returns to stock"
            .into(),
    ));
    Ok(())
}

/// Run Control4's own factory restore (menu.lst entry 0, the untouched
/// `/dev/sda2` restore system) ONCE, and reboot into it — the software
/// equivalent of holding the ID button.
///
/// Nothing is pushed from the host and sda2 is never written: the restore is the
/// box's own process, same kernel line as the button. What this writes is
/// `menu.lst` and `/boot/grub/default` on sda1 — see
/// [`hc::factory_once_menu`] for why it is a one-shot entry plus
/// `default saved`, and NOT `default 0` (which restores forever: restore.sh
/// reboots without touching menu.lst, and the next boot picks entry 0 again).
///
/// The default file is written and read back BEFORE menu.lst switches to
/// `default saved`, because GRUB 0.97 with `default saved` and no file falls
/// back to entry 0 — the same loop by another route.
pub fn factory_restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    let gm = "/mnt/ohc-grub";
    ssh.run(&format!("mkdir -p {gm} && mount {} {gm}", hc::GRUB_PART), true)
        .map_err(|e| anyhow::anyhow!("cannot mount {}: {e}", hc::GRUB_PART))?;
    let bail_umount = |msg: String| -> anyhow::Error {
        let _ = ssh.run(&format!("sync; umount {gm}"), false);
        anyhow::anyhow!(msg)
    };

    let menu = format!("{gm}/boot/grub/menu.lst");
    let dflt = format!("{gm}/boot/grub/default");
    let before = ssh
        .read_file(&menu)
        .ok_or_else(|| bail_umount(format!("{menu} is unreadable — refusing to write a bootloader blind")))?;
    let (out, once) = hc::factory_once_menu(&before).map_err(|e| bail_umount(e))?;

    let backup_suffix = format!(".pre-factory-restore.{}", chrono_like_stamp());
    ssh.run(&format!("cp {menu} {menu}{backup_suffix}"), true).map_err(|e| bail_umount(format!("{e}")))?;
    p.emit(Event::detail(format!("kept {menu}{backup_suffix}")));

    // 1. The default file, pointing at the one-shot entry. Rewrite only the
    //    first line if it exists (savedefault rewrites it in place by sector);
    //    create it padded if not.
    let have = ssh.test(&format!("-s {dflt}"));
    if have {
        ssh.run(&format!("sed -i '1s/.*/{once}/' {dflt}"), true).map_err(|e| bail_umount(format!("{e}")))?;
    } else {
        ssh.put_stream(hc::default_file(once).as_bytes(), &format!("cat > {dflt}"))
            .map_err(|e| bail_umount(format!("{e}")))?;
    }
    let first = ssh.run(&format!("sync; head -n 1 {dflt}"), true).map_err(|e| bail_umount(format!("{e}")))?;
    if first.trim() != once.to_string() {
        return Err(bail_umount(format!("{dflt} reads back {:?}, expected {once} — menu.lst NOT changed", first.trim())));
    }
    p.emit(Event::detail(format!("saved default -> entry {once} ({})", hc::FACTORY_ONCE_TITLE)));

    // 2. menu.lst, read back before trusting it.
    ssh.put_stream(out.as_bytes(), &format!("cat > {menu}")).map_err(|e| bail_umount(format!("{e}")))?;
    let after = ssh.read_file(&menu);
    if after.as_deref() != Some(out.as_str()) {
        let _ = ssh.run(&format!("cp {menu}{backup_suffix} {menu}"), false);
        return Err(bail_umount("menu.lst read back differently than written — restored the backup".into()));
    }
    p.emit(Event::detail(format!(
        "menu.lst verified, {} bytes: entry {once} restores once and saves entry {} as the default",
        out.len(),
        hc::ENTRY_VENDOR
    )));
    ssh.run(&format!("sync; umount {gm}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;

    p.emit(Event::step("rebooting into Control4's factory-restore system — this connection will drop".into()));
    let _ = ssh.run("sync; reboot", false);
    p.emit(Event::resolved(
        "on its way. The box runs Control4's own restore off the untouched recovery partition, \
         same as the ID button, then reboots into the freshly restored stock image. Allow ~5 minutes; \
         it comes back on a new DHCP lease, so find it by MAC"
            .into(),
    ));
    Ok(())
}

/// Cheap unique-enough suffix for a backup filename — no chrono dependency for
/// one string.
fn chrono_like_stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Remove an installed openHC and put the boot chain back the way it shipped.
///
/// The point of this is that it is a REAL revert, not a best effort: the
/// installer kept `menu.lst.pre-openhc` beside the file it edited, so what goes
/// back is the original bytes rather than a reconstruction. Only if that backup
/// is missing does it fall back to editing, and then it says so.
///
/// The kernel and initramfs on the kernel partition are removed too. `sda2` was
/// never written, so nothing there needs undoing.
pub fn uninstall(ssh: &Ssh, p: &Progress) -> Result<()> {
    let gm = "/mnt/ohc-grub";
    ssh.run(&format!("mkdir -p {gm} && mount {} {gm}", hc::GRUB_PART), true)
        .map_err(|e| anyhow::anyhow!("cannot mount {}: {e}", hc::GRUB_PART))?;
    let menu = format!("{gm}/boot/grub/menu.lst");
    let backup = format!("{menu}.pre-openhc");

    let have_backup = ssh.test(&format!("-s {backup}"));
    if have_backup {
        ssh.run(&format!("cp {backup} {menu}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;
        p.emit(Event::step("menu.lst restored from the pre-install backup".into()));
    } else {
        // No backup: drop our entry and put `default` back to the stock one.
        // Editing rather than restoring, which is worth saying out loud.
        let before = ssh.read_file(&menu).context("menu.lst is unreadable")?;
        let mut out = String::new();
        let mut in_ours = false;
        for l in before.lines() {
            if l.starts_with("title") {
                in_ours = l.contains("openHC");
            }
            if in_ours {
                continue;
            }
            if l.trim_start().starts_with("default") {
                out.push_str(&format!("default\t\t{}\n", hc::ENTRY_VENDOR));
            } else {
                out.push_str(l);
                out.push('\n');
            }
        }
        if out == before {
            let _ = ssh.run(&format!("umount {gm}"), false);
            p.emit(Event::warn("no openHC entry in menu.lst; nothing to remove".into()));
            return Ok(());
        }
        ssh.put_stream(out.as_bytes(), &format!("cat > {menu}"))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        p.emit(Event::warn(
            "no pre-install backup found — menu.lst was edited rather than restored".into(),
        ));
    }

    // Read back, and check the vendor's two guarded lines survived. Same rule
    // as the install: this partition does not get written on trust.
    let after = ssh.read_file(&menu).context("menu.lst unreadable after restoring it")?;
    for line in hc::GUARDED_LINES {
        if !after.lines().any(|l| l.trim_start().starts_with(line)) {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!("`{line}` is missing after the restore — do NOT reboot; menu.lst needs a look");
        }
    }
    if after.contains("title\t\topenHC") {
        let _ = ssh.run(&format!("umount {gm}"), false);
        bail!("the openHC entry is still in menu.lst after the restore");
    }
    // `default saved` is left alone deliberately when the backup restored it to
    // a number; if it is still `saved`, point it at the vendor entry so the
    // file it reads cannot outlive the entry it names.
    if after.lines().any(|l| l.trim_start().starts_with("default") && l.contains("saved")) {
        ssh.run(
            &format!("sed -i '1s/.*/{}/' {gm}/boot/grub/default 2>/dev/null; true", hc::ENTRY_VENDOR),
            false,
        )
        .ok();
        p.emit(Event::detail(format!("`default saved` kept, saved -> entry {}", hc::ENTRY_VENDOR)));
    } else {
        // `default saved` is gone with the entry, so the file it read is
        // orphaned. Only then: deleting it while menu.lst still says `default
        // saved` makes GRUB fall back to entry 0 — a factory restore on every
        // boot.
        let _ = ssh.run(&format!("rm -f {gm}/boot/grub/default"), false);
    }
    ssh.run(&format!("sync; umount {gm}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;

    // The images last: menu.lst no longer names them, so deleting them now
    // cannot leave an entry pointing at a file that is gone.
    let km = "/mnt/ohc-kernel";
    let _ = ssh.run(
        &format!(
            "mkdir -p {km} && mount {} {km} && rm -f {km}{} {km}{} && sync && umount {km}",
            hc::KERNEL_PART,
            hc::KERNEL_FILE,
            hc::INITRD_FILE
        ),
        false,
    );
    p.emit(Event::resolved(
        "removed. The boot chain is back to what Control4 shipped; nothing on this board was ever \
         written outside menu.lst and the spare kernel partition"
            .into(),
    ));
    Ok(())
}
