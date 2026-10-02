//! IO Extender install flows over SSH, from stock Control4 or from openHC.
//!
//! Every command here runs on stock's BusyBox 1.2.2 and mtd-utils as well as on
//! openHC's: no printf, no `head -c`, no `command -v`. See `core::iox` for where
//! the image goes and how U-Boot boots it.

use anyhow::{anyhow, bail, Context, Result};
use ohc_flash_core::board::Running;
use ohc_flash_core::iox::*;
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};
use crate::release::Release;

const STAGE: &str = "/tmp/ohc-stage";

fn run(ssh: &Ssh, cmd: &str) -> Result<String> {
    ssh.run(cmd, true).map_err(|e| anyhow!("{e}"))
}

/// Stock or openHC: openHC is the one with a board.env.
fn running(ssh: &Ssh) -> Running {
    if ssh.read_file("/opt/ohc/board.env").is_some() { Running::Openhc } else { Running::Stock }
}

fn image(rel: &Release) -> Result<&[u8]> {
    rel.get(IMAGE_FILE).with_context(|| format!("release has no {IMAGE_FILE}"))
}

/// Write openHC to the slot, verify it, and only then point U-Boot at it.
pub fn install_nand(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    let img = image(rel)?;
    check_image(img).map_err(|e| anyhow!(e))?;
    let os = running(ssh);
    let mtd = run(ssh, "cat /proc/mtd")?;
    let (dev, off) = slot(&mtd, os).map_err(|e| anyhow!(e))?;
    p.emit(Event::step(format!("running {os:?}; slot is {dev} @ {off:#x}")));

    // Stock's /tmp is a 16 MB ramdisk, too small for the image, so it gets a
    // tmpfs of its own. openHC's /tmp is already one.
    let staged = format!("{STAGE}/uImage");
    let mount = if os == Running::Stock {
        format!("grep -q ' {STAGE} ' /proc/mounts || mount -t tmpfs -o size=40m tmpfs {STAGE}")
    } else {
        "true".into()
    };
    run(ssh, &format!("mkdir -p {STAGE} && {mount}"))?;
    let pad = padded(img);
    p.emit(Event::step(format!("uploading {} bytes", pad.len())));
    ssh.put_stream(&pad, &format!("cat > {staged}")).map_err(|e| anyhow!("{e}"))?;
    let got: usize = run(ssh, &format!("wc -c < {staged}"))?.trim().parse().unwrap_or(0);
    if got != pad.len() {
        bail!("staged {got} bytes, expected {}", pad.len());
    }

    // nandwrite and U-Boot's nboot each skip a bad block, but nothing promises
    // they agree on where the image resumes after one. So: no bad blocks, or no
    // write.
    p.emit(Event::step(format!("erasing {SLOT_BLOCKS} blocks and writing")));
    let erase = run(ssh, &format!("flash_erase {dev} {off:#x} {SLOT_BLOCKS} 2>&1"))?;
    if erase.to_lowercase().contains("bad") {
        bail!("bad block in the slot; not writing an image U-Boot might misread:\n{erase}");
    }
    run(ssh, &format!("nandwrite -s {off:#x} {dev} {staged} >/dev/null"))?;

    // Read back through ECC and hash both sides on the box.
    let blocks = pad.len() as u64 / BLOCK;
    let want = run(ssh, &format!("md5sum {staged}"))?;
    let back = run(ssh, &format!(
        "dd if={dev} bs={BLOCK} skip={} count={blocks} 2>/dev/null | md5sum", off / BLOCK
    ))?;
    let (want, back) = (want.split_whitespace().next(), back.split_whitespace().next());
    if want.is_none() || want != back {
        bail!("read-back {back:?} does not match the staged image {want:?}; U-Boot's environment NOT changed");
    }
    run(ssh, &format!(
        "{}rm -rf {STAGE}",
        if os == Running::Stock { format!("umount {STAGE}; ") } else { String::new() }
    ))?;
    p.emit(Event::detail("image written and verified".into()));

    // Last, and bootcmd last of all: a failure above leaves U-Boot untouched.
    p.emit(Event::step("setting U-Boot's environment".into()));
    for (k, v) in env_settings() {
        run(ssh, &format!("fw_setenv {k} '{v}'"))?;
    }
    for (k, v) in env_settings() {
        let got = run(ssh, &format!("fw_printenv -n {k}"))?;
        if got.trim_end() != v {
            bail!("{k} reads back as {:?}, expected {v:?}", got.trim_end());
        }
    }
    p.emit(Event::detail("installed; restart the box to boot openHC from NAND".into()));
    Ok(())
}

/// The boot variables as U-Boot sees them.
pub fn status(ssh: &Ssh) -> Result<Vec<(String, String)>> {
    ["bootcmd", "ohc_try", "ohc_addr", "ohc_bootargs"]
        .iter()
        .map(|k| {
            let v = ssh.run(&format!("fw_printenv -n {k} 2>/dev/null"), false).map_err(|e| anyhow!("{e}"))?;
            Ok((k.to_string(), if v.trim().is_empty() { "(unset)".into() } else { v.trim().to_string() }))
        })
        .collect()
}

/// Boot the installed openHC again: clear the attempt count (after a fallback to
/// stock), make sure bootcmd is ours, and restart.
pub fn boot_installed(ssh: &Ssh, p: &Progress) -> Result<()> {
    if run(ssh, "fw_printenv -n ohcboot 2>/dev/null").unwrap_or_default().trim().is_empty() {
        bail!("openHC is not installed on this box (no ohcboot in U-Boot's environment)");
    }
    run(ssh, "fw_setenv ohc_try 0")?;
    run(ssh, &format!("fw_setenv bootcmd '{OPENHC_BOOTCMD}'"))?;
    p.emit(Event::step("restarting into openHC".into()));
    let _ = ssh.run("(sleep 1; reboot) >/dev/null 2>&1 &", false);
    Ok(())
}

/// Boot stock Control4 again. The openHC image stays in the slot (nothing reads
/// it), so `boot` can return to it without a reinstall.
pub fn uninstall(ssh: &Ssh, p: &Progress) -> Result<()> {
    run(ssh, &format!("fw_setenv bootcmd '{STOCK_BOOTCMD}'"))?;
    p.emit(Event::detail(format!("bootcmd is `{STOCK_BOOTCMD}`; restart the box to boot stock Control4")));
    Ok(())
}

/// Put the box back the way it shipped: U-Boot's factory `bootcmd`, none of the
/// variables openHC added, and the slot erased. Stock's own partitions were never
/// written, so nothing else needs restoring.
pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    let os = running(ssh);
    let mtd = run(ssh, "cat /proc/mtd")?;
    // bootcmd first: from here on U-Boot boots stock whatever else happens.
    p.emit(Event::step("setting U-Boot's factory bootcmd".into()));
    run(ssh, &format!("fw_setenv bootcmd '{FACTORY_BOOTCMD}'"))?;
    if run(ssh, "fw_printenv -n bootcmd")?.trim_end() != FACTORY_BOOTCMD.trim_end() {
        bail!("bootcmd did not read back as the factory value");
    }
    for v in NOT_FACTORY {
        run(ssh, &format!("fw_setenv {v}"))?;
    }
    p.emit(Event::detail(format!("removed {}", NOT_FACTORY.join(", "))));
    match slot(&mtd, os) {
        Ok((dev, off)) => {
            p.emit(Event::step(format!("erasing the openHC slot ({dev} @ {off:#x})")));
            run(ssh, &format!("flash_erase {dev} {off:#x} {SLOT_BLOCKS} >/dev/null 2>&1"))?;
        }
        // An older openHC with no "openhc" partition cannot reach the slot; the
        // image left there is inert (nothing boots it), so say so and carry on.
        Err(e) => p.emit(Event::warn(format!("slot not erased: {e}"))),
    }
    p.emit(Event::detail("factory boot restored; restart the box to boot stock Control4".into()));
    Ok(())
}
