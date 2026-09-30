//! The EA "netboot openHC into RAM" bring-up flow.
//!
//! This is the loop used to develop openHC on an EA before anything is written
//! to the box: it leaves the eMMC untouched and boots a kernel + initramfs
//! straight into RAM from CEFDK's unlocked manufacturing shell. It codifies the
//! prototype proven on hardware, three cooperating pieces driven in sequence:
//!
//!   1. a C4_COOKIE BOOTP responder (`:67`) that answers the box's request so
//!      CEFDK drops to `shell>` instead of auto-fetching a boot file;
//!   2. a read-only TFTP server (`:69`) serving the images dir, so the shell's
//!      `tftp get` pulls the kernel and initramfs;
//!   3. the serial console: wait for the shell (via the pure `MfgWatch` state
//!      machine), then type the CEFDK ramboot command sequence.
//!
//! The decisions — the exact shell commands, the RAM staging addresses, the
//! CEFDK globals — all live in `core::cefdk::ramboot_tftp_for`; this module only
//! sequences them and narrates progress.
//!
//! Binding `:67`/`:69` needs root, so this is run under sudo. That is not a bug
//! to work around; the servers just fail to bind otherwise, and the error says
//! so.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use ohc_flash_core::cefdk;
use ohc_flash_transport::{bootp, wait_for_login, BootpResponder, Serial, TftpServer, CEFDK_BAUD};

use crate::event::{Event, Progress};
use crate::mfgmode::{MfgWatch, Stage};
use crate::release::Release;

/// What CEFDK's `tftp get` asks for. These are the release's own artefact names,
/// staged under those exact names so the shell commands and the served files
/// agree by construction.
const KERNEL_NAME: &str = "bzImage";
const INITRD_NAME: &str = "rootfs.cpio.gz";

/// How long to wait to reach the CEFDK shell before giving up. Generous: the
/// user may need a retry or two to get the ID button held through a power cycle,
/// and `MfgWatch` narrates each attempt.
const SHELL_DEADLINE: Duration = Duration::from_secs(300);

/// How long to wait for the box to answer SSH after `bootlinux`. openHC brings
/// up the network, mounts the initramfs and starts dropbear; ~4 min covers a
/// cold RAM boot with margin.
const LOGIN_WAIT_SECS: u64 = 240;

/// Netboot openHC into RAM over serial + TFTP.
///
/// `serial_dev` is the CEFDK console (e.g. `/dev/tty.usbserial-XXXX`).
/// `server_ip` is this host on the bring-up link; `box_ip` is the address CEFDK
/// takes for the transfer (also the BOOTP offer). `mac` is the target's, so we
/// answer only it. `cmdline` is the RAM-boot kernel command line (the CLI
/// defaults it to [`cefdk::RAMBOOT_CMDLINE`]). With `wait`, the flow blocks
/// until the box answers SSH.
pub fn netboot(
    serial_dev: &str,
    images: &Release,
    server_ip: &str,
    box_ip: &str,
    mac: &str,
    cmdline: &str,
    wait: bool,
    p: &Progress,
) -> Result<()> {
    let server_ip: Ipv4Addr = server_ip.parse().with_context(|| format!("bad --server-ip {server_ip}"))?;
    let box_ip: Ipv4Addr = box_ip.parse().with_context(|| format!("bad --box-ip {box_ip}"))?;
    let target_mac = bootp::parse_mac(mac).ok_or_else(|| anyhow!("bad --mac {mac:?} (want aa:bb:cc:dd:ee:ff)"))?;

    // The initrd length handed to bootlinux MUST be the exact byte count of
    // rootfs.cpio.gz: the kernel's gzip/cpio reader is given this as the ramdisk
    // size and needs the whole image (a bzImage self-describes its end; a cpio.gz
    // does not, so an over- or under-count corrupts the rootfs).
    let kernel = images.get(KERNEL_NAME).context("release has no bzImage")?;
    let initrd = images.get(INITRD_NAME).context("release has no rootfs.cpio.gz")?;
    let initrd_len = initrd.len() as u64;

    // Serve the two images by their canonical names. Staging from the in-memory
    // Release (rather than pointing TFTP at the source) makes a dir and a .zip
    // release behave identically and guarantees the served names match the shell
    // commands.
    let stage = StageDir::new()?;
    stage.write(KERNEL_NAME, kernel)?;
    stage.write(INITRD_NAME, initrd)?;
    p.emit(Event::detail(format!(
        "staged {KERNEL_NAME} ({} B) and {INITRD_NAME} ({initrd_len} B) for TFTP",
        kernel.len()
    )));

    // 1. Start the two servers. Bind failures here are almost always "not root".
    let bootp_p = p.clone();
    let bootp = BootpResponder::start(target_mac, box_ip, server_ip, move |m| {
        bootp_p.emit(Event::detail(m))
    })
    .map_err(|e| bind_error("BOOTP", 67, e))?;
    let tftp_p = p.clone();
    let tftp = TftpServer::start(stage.path().to_path_buf(), move |m| tftp_p.emit(Event::detail(m)))
        .map_err(|e| bind_error("TFTP", 69, e))?;
    p.emit(Event::step(format!(
        "serving {KERNEL_NAME}/{INITRD_NAME} over TFTP and answering BOOTP for {}",
        mac
    )));

    // 2. Drive the serial console to the shell.
    let mut serial = Serial::open(serial_dev, CEFDK_BAUD).map_err(|e| anyhow!("{e}"))?;
    p.emit(Event::detail(format!("opened {serial_dev} at {CEFDK_BAUD} 8N1")));
    let reached = drive_to_shell(&mut serial, p)?;
    if !reached {
        // Stop the servers before returning (Drop would anyway, but be explicit).
        bootp.stop();
        tftp.stop();
        bail!(
            "did not reach the CEFDK shell within {}s — check the serial cable and that the ID \
             button was held through the power cycle",
            SHELL_DEADLINE.as_secs()
        );
    }

    // 3. At the shell: type the ramboot sequence. core owns the exact commands.
    let cmds = cefdk::ramboot_tftp_for(
        &server_ip.to_string(),
        &box_ip.to_string(),
        KERNEL_NAME,
        INITRD_NAME,
        initrd_len,
        cmdline,
    );
    send_ramboot(&mut serial, &cmds, p)?;
    p.emit(Event::resolved("ramboot sequence sent — the box is booting openHC into RAM".into()));

    // The servers have done their job once bootlinux runs; keep them alive until
    // now (the TFTP transfer happens while we type), then stop.
    bootp.stop();
    tftp.stop();

    // 4. Optionally wait for the box to come up on the network.
    if wait {
        p.emit(Event::step(format!("waiting up to {LOGIN_WAIT_SECS}s for openHC to answer SSH at {box_ip}")));
        match wait_for_login(&box_ip.to_string(), LOGIN_WAIT_SECS) {
            Some(_) => p.emit(Event::resolved(format!("openHC is up at {box_ip}"))),
            None => p.emit(Event::warn(format!(
                "no SSH from {box_ip} within {LOGIN_WAIT_SECS}s. It may still be booting, or came \
                 up on a different address — watch the serial console for its login."
            ))),
        }
    }
    Ok(())
}

/// Read the console, feeding `MfgWatch`, until it reports the shell — or the
/// deadline passes. Returns whether the shell was reached.
///
/// The read/tick rhythm mirrors what the pure state machine expects: complete
/// lines are fed as they arrive, and on a silent window the un-terminated tail
/// is fed too (that is how the newline-less `shell>` prompt is seen) before
/// `tick` reports the silence.
fn drive_to_shell(serial: &mut Serial, p: &Progress) -> Result<bool> {
    let mut watch = MfgWatch::new();
    watch.prompt(p);

    let start = Instant::now();
    while start.elapsed() < SHELL_DEADLINE {
        let lines = serial.read_lines(Duration::from_millis(500)).map_err(|e| anyhow!("{e}"))?;
        if lines.is_empty() {
            // Nothing completed a line this window. The prompt has no newline, so
            // pull the tail and feed it once so `shell>` is not missed.
            if let Some(tail) = serial.take_pending() {
                if watch.feed(&tail, p) == Some(Stage::AtShell) {
                    return Ok(true);
                }
            }
            watch.tick(start.elapsed().as_secs(), p);
            continue;
        }
        for line in lines {
            if watch.feed(&line, p) == Some(Stage::AtShell) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Type each ramboot command at the shell, echoing it as a Detail so the log
/// shows exactly what was sent. A small gap between lines lets CEFDK's line
/// editor and each command (notably the two `tftp get` transfers) keep up — the
/// shell has no flow control, so a burst can drop characters.
fn send_ramboot(serial: &mut Serial, cmds: &[String], p: &Progress) -> Result<()> {
    for cmd in cmds {
        p.emit(Event::detail(format!("shell> {cmd}")));
        serial.write_line(cmd).map_err(|e| anyhow!("{e}"))?;
        // The two tftp transfers take longer than a poke; the fixed gap is a
        // floor, and the box's own output (surfaced by the TFTP server's
        // announce) shows the transfer proceeding.
        std::thread::sleep(inter_command_gap(cmd));
    }
    Ok(())
}

/// How long to pause after a command before sending the next. A `tftp get`
/// pulls megabytes and must finish before the following line is typed, or that
/// line lands in the middle of the transfer; everything else is a register poke
/// that returns immediately.
fn inter_command_gap(cmd: &str) -> Duration {
    if cmd.starts_with("tftp get") {
        Duration::from_secs(3)
    } else {
        Duration::from_millis(300)
    }
}

/// Turn a bind failure into a message that names the likely cause. Binding the
/// privileged ports is the one thing that fails predictably, and "Permission
/// denied" with no context sends people down the wrong path.
fn bind_error(what: &str, port: u16, e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        anyhow!("cannot bind the {what} server on :{port}: {e}. Binding a port below 1024 needs \
                 root — run this under sudo.")
    } else if e.kind() == std::io::ErrorKind::AddrInUse {
        anyhow!("cannot bind the {what} server on :{port}: {e}. Another DHCP/TFTP service (or a \
                 previous run) already holds it — stop it and retry.")
    } else {
        anyhow!("cannot bind the {what} server on :{port}: {e}")
    }
}

/// A throwaway directory the images are staged into for TFTP, removed on drop.
///
/// The engine has no `tempfile` dependency and does not need one for this: a
/// pid+nanos name under the OS temp dir is unique enough for a bring-up tool,
/// and Drop cleans it up even if the flow errors out.
struct StageDir {
    path: PathBuf,
}

impl StageDir {
    fn new() -> Result<StageDir> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("ohc-netboot-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)
            .with_context(|| format!("creating stage dir {}", path.display()))?;
        Ok(StageDir { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, data: &[u8]) -> Result<()> {
        std::fs::write(self.path.join(name), data)
            .with_context(|| format!("staging {name} for TFTP"))
    }
}

impl Drop for StageDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tftp_gets_the_long_gap() {
        // The transfer commands must be given time to finish; a poke must not,
        // or the whole sequence crawls.
        assert!(inter_command_gap("tftp get 10.0.0.106 0x6000000 bzImage") >= Duration::from_secs(1));
        assert!(inter_command_gap("cache flush") < Duration::from_secs(1));
        assert!(inter_command_gap("ord4 0xc90a4 = 0x6000000") < Duration::from_secs(1));
    }

    #[test]
    fn the_ramboot_default_carries_its_ea_quirks() {
        // routeirq/nocrs are load-bearing on the EA and this is what the CLI
        // defaults --cmdline to; assert they survive so a refactor of the
        // default cannot silently drop them.
        let c = cefdk::RAMBOOT_CMDLINE;
        assert!(c.contains("routeirq"), "EA SoC UARTs need routeirq: {c}");
        assert!(c.contains("nocrs"), "CEFDK E820 omits PCI windows: {c}");
    }

    /// The stage dir is what TFTP serves; the whole transfer fails if the files
    /// are not written under their exact requested names. Prove the round trip
    /// and that cleanup happens.
    #[test]
    fn stage_dir_writes_named_files_and_cleans_up() {
        let path;
        {
            let s = StageDir::new().expect("temp dir");
            path = s.path().to_path_buf();
            s.write(KERNEL_NAME, b"kernel-bytes").unwrap();
            s.write(INITRD_NAME, b"initrd-bytes").unwrap();
            assert_eq!(std::fs::read(path.join(KERNEL_NAME)).unwrap(), b"kernel-bytes");
            assert_eq!(std::fs::read(path.join(INITRD_NAME)).unwrap(), b"initrd-bytes");
        }
        assert!(!path.exists(), "stage dir must be removed on drop");
    }
}
