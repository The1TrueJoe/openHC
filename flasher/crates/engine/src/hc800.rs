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

/// Where images are staged on the box. A tmpfs on openHC; real disk on the
/// stock image, which is why [`kexec`] cleans up after itself there.
const STAGE: &str = "/tmp";

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

/// Start openHC out of the running system. **Writes to no partition.**
///
/// `netconsole` is worth passing whenever the caller knows where to listen: the
/// window between `kexec -e` and the new kernel bringing up the NIC is the only
/// part of this that a serial cable can see and SSH cannot.
pub fn kexec(ssh: &Ssh, rel: &Release, netconsole: Option<&str>, p: &Progress) -> Result<()> {
    let (kp, ip) = stage(ssh, rel, p)?;

    // The vendor image's kernel is CONFIG_KEXEC=y but Control4 never shipped
    // the userspace tool, so a stock box needs one pushed. openHC has its own.
    let kexec_bin = if ssh.run("command -v kexec", false).map(|s| !s.trim().is_empty()).unwrap_or(false)
    {
        "kexec".to_string()
    } else {
        let staged = format!("{STAGE}/kexec-i686-static");
        if ssh.run(&format!("test -x {staged}"), false).is_err() {
            bail!(
                "no kexec on this system. The stock Control4 image has none; build the static \
                 i686 one (.github/workflows/tools.yml) and put it at {staged}"
            );
        }
        staged
    };

    // Stop our own watchdog before the handover. kexec -e runs no shutdown
    // script, so a kicker left running would be killed mid-jump WITHOUT the
    // magic close — which is precisely how you arm the timer rather than
    // disarm it. Harmless on the stock image, which has no such script.
    let _ = ssh.run("/etc/init.d/S02watchdog stop 2>/dev/null; true", false);

    let mut append = hc::CMDLINE.to_string();
    if let Some(nc) = netconsole {
        append.push(' ');
        append.push_str(nc);
        p.emit(Event::detail(format!("netconsole: {nc}")));
    }

    p.emit(Event::step("kexec -l (staging into the running kernel)".into()));
    ssh.run(&format!("{kexec_bin} -l {kp} --initrd={ip} --append='{append}'"), true)
        .map_err(|e| anyhow::anyhow!("kexec -l refused the image: {e}"))?;

    p.emit(Event::step("kexec -e — this connection will drop".into()));
    // The box goes away mid-command, so a non-zero exit here is the expected
    // outcome and not an error worth reporting.
    let _ = ssh.run(&format!("sync; {kexec_bin} -e"), false);
    p.emit(Event::resolved(
        "openHC is starting. It takes a fresh DHCP lease, so find it by MAC, not by its old address"
            .into(),
    ));
    Ok(())
}

/// The persistent install. This is the only flow in the tool that writes the
/// bootloader partition, so it checks before it does and reads back after.
pub fn install_grub(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
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
    let before = ssh
        .read_file(&menu)
        .with_context(|| format!("{menu} is unreadable — refusing to write a bootloader blind"))?;

    // The two lines Control4 patched in are what the hardware factory-default
    // button reads. If they are not where we expect, this is not the menu.lst
    // this tool was written against and it should not be editing it.
    for line in hc::GUARDED_LINES {
        if !before.lines().any(|l| l.trim_start().starts_with(line)) {
            let _ = ssh.run(&format!("umount {gm}"), false);
            bail!("menu.lst has no `{line}` line — the factory-default button depends on it");
        }
    }
    if before.contains("title\t\topenHC") || before.contains("title openHC") {
        p.emit(Event::warn("menu.lst already has an openHC entry; replacing the images only".into()));
        ssh.run(&format!("sync; umount {gm}"), true).map_err(|e| anyhow::anyhow!("{e}"))?;
        return Ok(());
    }

    // One backup on the box itself, alongside the one that should already be on
    // the operator's machine. Cheap, and the file is ~1 KB.
    ssh.run(&format!("cp {menu} {menu}.pre-openhc"), true).map_err(|e| anyhow::anyhow!("{e}"))?;
    p.emit(Event::detail(format!("kept {menu}.pre-openhc")));

    // `default N` -> `default saved`, which is what turns the entry's
    // `savedefault` into a boot-once. Only the default line changes; the two
    // vendor entries are appended past, never rewritten.
    let mut out = String::new();
    let mut swapped = false;
    for l in before.lines() {
        if l.trim_start().starts_with("default") && !l.contains("saved") {
            out.push_str("default\t\tsaved\n");
            swapped = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !swapped {
        let _ = ssh.run(&format!("umount {gm}"), false);
        bail!("menu.lst has no `default` line to convert");
    }
    out.push_str(&hc::menu_entry());

    ssh.put_stream(out.as_bytes(), &format!("cat > {menu}")).map_err(|e| anyhow::anyhow!("{e}"))?;
    ssh.put_stream(
        hc::default_file(hc::ENTRY_OPENHC).as_bytes(),
        &format!("cat > {gm}/boot/grub/default"),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

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

    p.emit(Event::resolved(format!(
        "installed. The next boot runs openHC; every openHC boot then re-points the default at \
         entry {} (Control4), so a reset of any kind returns to stock",
        hc::ENTRY_VENDOR
    )));
    Ok(())
}
