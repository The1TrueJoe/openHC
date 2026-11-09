//! HC-250 flows over SSH: RAM boot from stock, the SD install, and the way back.
//!
//! The SD install crosses three systems and two reboots, because each half has
//! one place it can run: the SD and EEPROM are written from openHC (stock's
//! kernel does not even power the card), and U-Boot's env only by stock's own
//! fw_setenv (Linux's NAND ECC is not Control4's). See `core::hc250`.
//!
//! Stock's BusyBox 1.15 has no `nohup`: detached jobs are `( ... ) &`.

use anyhow::{anyhow, bail, Context, Result};
use ohc_flash_core::board::Running;
use ohc_flash_core::hc250::*;
use ohc_flash_transport::ssh::{wait_for_login, Ssh};

use crate::event::{Event, Progress};
use crate::release::Release;

/// Where the stock-side kit is staged on either system (tmpfs).
const KIT: &str = "/tmp/ohc-kit";
/// How long a reboot or a kexec may take to answer SSH again.
const BOOT_WAIT: u64 = 300;

fn run(ssh: &Ssh, cmd: &str) -> Result<String> {
    ssh.run(cmd, true).map_err(|e| anyhow!("{e}"))
}

/// openHC writes /etc/openhc-release; stock does not.
fn running(ssh: &Ssh) -> Running {
    if ssh.read_file("/etc/openhc-release").is_some() { Running::Openhc } else { Running::Stock }
}

fn from_sd(ssh: &Ssh) -> bool {
    ssh.read_file("/proc/cmdline").is_some_and(|c| booted_from_sd(&c))
}

fn file<'a>(rel: &'a Release, name: &str) -> Result<&'a [u8]> {
    rel.get(name).with_context(|| format!("release has no {name} (an HC-250 bundle carries it)"))
}

fn image(rel: &Release) -> Result<&[u8]> {
    let img = file(rel, IMAGE_FILE)?;
    check_image(img).map_err(|e| anyhow!(e))?;
    Ok(img)
}

/// Copy one kit file to [`KIT`] on the box.
fn put_kit(ssh: &Ssh, rel: &Release, name: &str, exec: bool) -> Result<String> {
    let path = format!("{KIT}/{name}");
    let chmod = if exec { format!(" && chmod +x {path}") } else { String::new() };
    ssh.put_stream(file(rel, name)?, &format!("mkdir -p {KIT} && cat > {path}{chmod}"))
        .map_err(|e| anyhow!("{e}"))?;
    Ok(path)
}

/// Reboot detached and wait for the box to answer again, whichever system that is.
fn reboot_and_wait(ssh: &Ssh, what: &str, p: &Progress) -> Result<Ssh> {
    p.emit(Event::step(format!("rebooting into {what} — this connection will drop")));
    let _ = ssh.run("sync; (sleep 1; reboot) </dev/null >/dev/null 2>&1 &", false);
    wait_for_login(&ssh.host, BOOT_WAIT).with_context(|| format!("{} did not come back as {what}", ssh.host))
}

/// Start openHC in RAM from stock: nothing is written, a reboot returns to stock.
pub fn kexec(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    if running(ssh) != Running::Stock {
        bail!("kexec starts from stock Control4; this box runs openHC already");
    }
    kexec_supported(&run(ssh, "uname -r")?).map_err(|e| anyhow!(e))?;
    let img = image(rel)?;
    put_kit(ssh, rel, KEXEC_KO, false)?;
    let sh = put_kit(ssh, rel, KEXEC_SH, true)?;
    p.emit(Event::step(format!("streaming {} bytes into /dev/ohc-kexec", img.len())));
    let out = ssh.put_stream(img, &format!("{sh} - load 2>&1")).map_err(|e| anyhow!("{e}"))?;
    if !out.contains("loaded") {
        bail!("ohc-kexec did not load the image:\n{}", out.trim());
    }
    p.emit(Event::step("jumping into openHC — this connection will drop".into()));
    let _ = ssh.run(
        "sync; (sleep 1; echo 1 > /sys/module/ohc_kexec/parameters/go) </dev/null >/dev/null 2>&1 &",
        false,
    );
    p.emit(Event::resolved(
        "openHC is starting from RAM (about 30 s; same MAC, so usually the same address). A reboot \
         returns the box to stock"
            .into(),
    ));
    Ok(())
}

/// Stock: the env half (bootohc, the restore hook, bootmode=ohc), and stock's IO
/// firmware tree for the way back.
fn stock_half(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<Vec<u8>> {
    let env = put_kit(ssh, rel, BOOT_ENV_SH, true)?;
    p.emit(Event::step("writing U-Boot's env with stock's fw_setenv".into()));
    let out = run(ssh, &format!("{env} install 2>&1"))?;
    for l in out.lines().filter(|l| !l.starts_with("crc") && !l.trim().is_empty()) {
        p.emit(Event::detail(l.trim().to_string()));
    }
    let st = run(ssh, &format!("{env} status"))?;
    if bootmode(&st) != Some("ohc") {
        bail!("bootmode did not read back as ohc:\n{}", st.trim());
    }
    p.emit(Event::step("saving stock's IO firmware tree (for `restore`)".into()));
    let tar = ssh.run_bytes(&format!("{env} stock-io")).map_err(|e| anyhow!("{e}"))?;
    if tar.len() < 1024 {
        bail!("stock's IO firmware tree came back as {} bytes", tar.len());
    }
    Ok(tar)
}

/// The persistent install, from stock or from openHC. Ends with the box running
/// openHC from the SD card.
pub fn install_sd(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    let img = image(rel)?;
    file(rel, BOOT_ENV_SH)?;
    let mut box_ = ssh.clone();

    // openHC already on the SD: a new image is all an update needs.
    if running(&box_) == Running::Openhc && from_sd(&box_) {
        p.emit(Event::step(format!("updating {SD_IMAGE} on the SD ({} bytes)", img.len())));
        let out = box_.put_stream(img, "ohc-sd-install install - 2>&1").map_err(|e| anyhow!("{e}"))?;
        p.emit(Event::detail(out.trim().lines().last().unwrap_or("").to_string()));
        p.emit(Event::resolved("image updated; restart the box to run it".into()));
        return Ok(());
    }

    // 1. Into openHC in RAM (from stock), saving stock's IO tree on the way.
    let mut io_tar = None;
    if running(&box_) == Running::Stock {
        io_tar = Some(stock_half(&box_, rel, p)?);
        kexec(&box_, rel, p)?;
        box_ = wait_for_login(&box_.host, BOOT_WAIT).context("openHC did not come up from RAM")?;
        if running(&box_) != Running::Openhc {
            bail!("expected openHC in RAM after the kexec; the box answers as stock");
        }
    }

    // 2. openHC: the SD, the image, the EEPROM try count.
    p.emit(Event::step(format!("partitioning the SD (once) and writing {SD_IMAGE} ({} bytes)", img.len())));
    let out = box_.put_stream(img, "ohc-sd-install install - 2>&1").map_err(|e| anyhow!("{e}"))?;
    for l in out.lines().filter(|l| !l.trim().is_empty() && !l.contains('%')) {
        p.emit(Event::detail(l.trim().to_string()));
    }

    // 3. Stock, for the env (skipped when stock already did it in step 1).
    if io_tar.is_none() {
        box_ = reboot_and_wait(&box_, "stock (a RAM boot returns there)", p)?;
        if running(&box_) != Running::Stock {
            bail!("expected stock after rebooting the RAM boot; is openHC already installed?");
        }
        io_tar = Some(stock_half(&box_, rel, p)?);
    }

    // 4. Into openHC from the SD, and keep stock's IO tree there.
    box_ = reboot_and_wait(&box_, "openHC from the SD", p)?;
    if !(running(&box_) == Running::Openhc && from_sd(&box_)) {
        bail!("the box did not boot openHC from the SD (U-Boot fell back to stock?)");
    }
    if let Some(tar) = io_tar {
        let out = box_.put_stream(&tar, "ohc-sd-install stock-io 2>&1").map_err(|e| anyhow!("{e}"))?;
        p.emit(Event::detail(out.trim().lines().last().unwrap_or("").to_string()));
    }
    p.emit(Event::resolved(format!(
        "openHC runs from the SD card. {MAX_TRIES} failed boots in a row fall back to stock; \
         `ohc-flash restore` or the factory-restore button returns the box to stock"
    )));
    Ok(())
}

/// What the box says about its install.
pub fn status(ssh: &Ssh, rel: Option<&Release>) -> Result<Vec<(String, String)>> {
    let mut out = vec![("running".to_string(), format!("{:?}", running(ssh)))];
    if running(ssh) == Running::Openhc {
        out.push(("booted from".into(), if from_sd(ssh) { "SD".into() } else { "RAM".into() }));
        if let Some(bin) = crate::restore_bin(ssh) {
            let st = run(ssh, &format!("{bin} status"))?;
            out.push(("restore".into(), restore_state(&st).unwrap_or("?").to_string()));
        }
    } else if let Some(rel) = rel {
        let env = put_kit(ssh, rel, BOOT_ENV_SH, true)?;
        let st = run(ssh, &format!("{env} status"))?;
        out.push(("bootmode".into(), bootmode(&st).unwrap_or("?").to_string()));
    }
    Ok(out)
}

/// Back into the installed openHC: re-arm the try count (a fallback to stock
/// leaves it at 3, which only openHC can clear) and reboot.
pub fn boot_installed(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    let mut box_ = ssh.clone();
    if running(&box_) == Running::Stock {
        let env = put_kit(&box_, rel, BOOT_ENV_SH, true)?;
        if bootmode(&run(&box_, &format!("{env} status"))?) != Some("ohc") {
            bail!("openHC is not installed on this box (bootmode is not ohc); use `install`");
        }
        kexec(&box_, rel, p)?;
        box_ = wait_for_login(&box_.host, BOOT_WAIT).context("openHC did not come up from RAM")?;
    }
    run(&box_, "ohc-sd-install arm")?;
    reboot_and_wait(&box_, "openHC from the SD", p)?;
    p.emit(Event::resolved("openHC runs from the SD again".into()));
    Ok(())
}

/// Boot stock again, keeping the SD: every boot goes to stock (EEPROM), then
/// stock's env goes back to bootmode=nand.
pub fn uninstall(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    let mut box_ = ssh.clone();
    if running(&box_) == Running::Openhc {
        run(&box_, "ohc-sd-install stock")?;
        box_ = reboot_and_wait(&box_, "stock", p)?;
        if running(&box_) != Running::Stock {
            bail!("expected stock after `ohc-sd-install stock`");
        }
    }
    let env = put_kit(&box_, rel, BOOT_ENV_SH, true)?;
    let out = run(&box_, &format!("{env} uninstall 2>&1"))?;
    p.emit(Event::detail(out.lines().filter(|l| !l.starts_with("crc")).collect::<Vec<_>>().join("\n")));
    p.emit(Event::resolved(
        "U-Boot boots stock from the NAND; openHC stays on the SD for `install` to point back at".into(),
    ));
    Ok(())
}

/// Control4's factory restore, from openHC: Control4's IO firmware back on the
/// microcontroller, then U-Boot runs the restore on the next boot.
pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    if running(ssh) == Running::Stock {
        bail!(
            "this box runs stock already. Its factory-restore button reimages it; `uninstall` stops \
             it booting openHC"
        );
    }
    let bin = crate::restore_bin(ssh).context("this openHC has no restore tool (an image without the restore feature)")?;
    p.emit(Event::step("putting Control4's IO firmware back, then arming the factory restore".into()));
    let out = ssh
        .run(&format!("{bin} stock --no-reboot 2>&1"), true)
        .map_err(|e| anyhow!("restore stock failed, nothing rebooted: {e}"))?;
    for l in out.lines().filter(|l| !l.trim().is_empty()) {
        p.emit(Event::detail(l.trim().to_string()));
    }
    p.emit(Event::step("rebooting into Control4's factory restore — this connection will drop".into()));
    let _ = ssh.run("sync; (sleep 1; reboot) </dev/null >/dev/null 2>&1 &", false);
    p.emit(Event::resolved(
        "U-Boot runs Control4's restore (the recovery image, about 4-5 minutes) and boots stock. \
         `ohc-flash install` points it back at the openHC still on the SD"
            .into(),
    ));
    Ok(())
}
