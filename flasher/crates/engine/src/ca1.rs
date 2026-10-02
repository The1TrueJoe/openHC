//! CA-1 return-to-factory over SSH. See `core::ca1` for why it is a one-shot
//! `factoryrestore` and how the one-shot is built.

use anyhow::{anyhow, bail, Context, Result};
use ohc_flash_core::ca1::*;
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};

const CFG: &str = "/tmp/ohc-fw_env.config";
const FAT_MNT: &str = "/tmp/ohc-fat";

fn run(ssh: &Ssh, cmd: &str) -> Result<String> {
    ssh.run(cmd, true).map_err(|e| anyhow!("{e}"))
}

/// Read a device as bytes. `Ssh::run` returns text, so it travels as hex.
fn read_bytes(ssh: &Ssh, dev: &str) -> Result<Vec<u8>> {
    let hex = run(ssh, &format!("od -An -tx1 -v {dev}"))?;
    hex.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).map_err(|e| anyhow!("{dev}: {e}")))
        .collect()
}

/// `fw_printenv`/`fw_setenv` invocations that address the right flash. Stock
/// ships a valid /etc/fw_env.config; openHC's is measured here, from the
/// environment itself, and passed with `-c`.
fn env_tools(ssh: &Ssh, p: &Progress) -> Result<(String, String)> {
    if ssh.read_file("/opt/ohc/board.env").is_none() {
        return Ok(("fw_printenv".into(), "fw_setenv".into()));
    }
    let mtd = run(ssh, "cat /proc/mtd")?;
    let (a, b) = env_devices(&mtd).context("no U-Boot environment partitions in /proc/mtd")?;
    let geo = [&a, &b]
        .iter()
        .find_map(|d| read_bytes(ssh, d).ok().and_then(|c| env_geometry(&c)))
        .context("neither U-Boot environment copy has a valid CRC; refusing to write it")?;
    p.emit(Event::detail(format!("U-Boot environment: {:#x} bytes, header {}", geo.0, geo.1)));
    ssh.put_stream(fw_env_config(&a, &b, geo.0).as_bytes(), &format!("cat > {CFG}"))
        .map_err(|e| anyhow!("{e}"))?;
    Ok((format!("fw_printenv -c {CFG}"), format!("fw_setenv -c {CFG}")))
}

fn get(ssh: &Ssh, pr: &str, k: &str) -> String {
    ssh.run(&format!("{pr} -n {k} 2>/dev/null"), false).unwrap_or_default().trim_end().to_string()
}

fn set(ssh: &Ssh, pr: &str, se: &str, k: &str, v: &str) -> Result<()> {
    if v.contains('\'') {
        bail!("{k}: value holds a single quote; not passing it through a shell");
    }
    run(ssh, &format!("{se} {k} '{v}'"))?;
    if get(ssh, pr, k) != v {
        bail!("{k} did not read back as written");
    }
    Ok(())
}

/// Delete openHC's files from the FAT partition and make the next boot run
/// Control4's `factoryrestore` once. On a box already restored, fold `bootcmd`
/// back to the literal original and remove the helper variables.
pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    let (pr, se) = env_tools(ssh, p)?;
    if get(ssh, &pr, "factoryrestore").is_empty() {
        bail!("this U-Boot has no `factoryrestore` command; nothing changed");
    }

    // Second run, on the restored stock box: tidy the environment.
    let held = get(ssh, &pr, STOCK_HOLD);
    if !held.is_empty() && get(ssh, &pr, "bootcmd") == HELD_BOOTCMD {
        set(ssh, &pr, &se, "bootcmd", &held)?;
        run(ssh, &format!("{se} {STOCK_HOLD}; {se} {ONCE}"))?;
        p.emit(Event::detail("bootcmd is the factory original again; helper variables removed".into()));
        return Ok(());
    }

    let orig = get(ssh, &pr, "bootcmd");
    if orig.is_empty() || orig.starts_with("run ohc_") {
        bail!("bootcmd is {orig:?}; a restore is already pending or the environment is unreadable");
    }

    // Our files first: from here on, even without the recovery, U-Boot takes the
    // stock zImage path instead of our boot.scr.
    p.emit(Event::step(format!("removing {} from {FAT_PART}", OUR_FILES.join(", "))));
    let rm = OUR_FILES.iter().map(|f| format!("{FAT_MNT}/{f}")).collect::<Vec<_>>().join(" ");
    run(ssh, &format!(
        "mkdir -p {FAT_MNT} && mount -t vfat {FAT_PART} {FAT_MNT} && rm -f {rm}; sync; umount {FAT_MNT}"
    ))?;

    p.emit(Event::step("arming a one-shot `factoryrestore` in U-Boot".into()));
    set(ssh, &pr, &se, STOCK_HOLD, &orig)?;
    set(ssh, &pr, &se, ONCE, ONCE_SCRIPT)?;
    set(ssh, &pr, &se, "bootcmd", ONCE_BOOTCMD)?;

    p.emit(Event::step("restarting into Control4's recovery".into()));
    let _ = ssh.run("(sleep 1; reboot) >/dev/null 2>&1 &", false);
    p.emit(Event::detail(
        "the recovery re-images the box from p3 (~3 min) and boots stock; run `restore` once more \
         against it to tidy U-Boot's environment"
            .into(),
    ));
    Ok(())
}
