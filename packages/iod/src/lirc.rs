//! The kernel's lirc devices. One node per emitter, so the node IS the port.
//!
//! Found by NAME, never by number: `/dev/lirc3` is whatever probe order made
//! it, and with a USB IR dongle plugged in it may not be ours at all.
use std::io;
use std::path::{Path, PathBuf};

/// One lirc node and the label the driver gave it.
#[derive(Clone, Debug)]
pub struct Dev {
    pub path: PathBuf,
    pub name: String,
}

/// Every lirc device, with its `DEV_NAME`. Reading the owning rc device's
/// `uevent` runs rc-core's hook, which is the only place the label is exposed.
#[cfg(target_os = "linux")]
pub fn devices() -> Vec<Dev> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/lirc") else {
        return out;
    };
    for e in dir.flatten() {
        let node = e.file_name();
        let node = node.to_string_lossy();
        if !node.starts_with("lirc") {
            continue;
        }
        let uevent = e.path().join("device/uevent");
        let name = std::fs::read_to_string(&uevent)
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("DEV_NAME=").map(str::to_string))
            })
            .unwrap_or_default();
        out.push(Dev { path: PathBuf::from("/dev").join(&*node), name });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(not(target_os = "linux"))]
pub fn devices() -> Vec<Dev> {
    Vec::new()
}

pub fn find(name: &str) -> Option<Dev> {
    devices().into_iter().find(|d| d.name == name)
}

pub fn present() -> bool {
    devices().iter().any(|d| d.name.starts_with("openHC IR"))
}

// linux/lirc.h: _IOW('i', n, __u32).
#[cfg(target_os = "linux")]
const LIRC_SET_SEND_CARRIER: libc::c_ulong = 0x4004_6913;
#[cfg(target_os = "linux")]
const LIRC_SET_REC_MODE: libc::c_ulong = 0x4004_6912;
#[cfg(target_os = "linux")]
const LIRC_MODE_MODE2: u32 = 0x0000_0004;

pub const MODE2_PULSE: u32 = 0x0100_0000;
pub const MODE2_FREQUENCY: u32 = 0x0200_0000;
pub const MODE2_TIMEOUT: u32 = 0x0300_0000;
pub const MODE2_MASK: u32 = 0xFF00_0000;
pub const VALUE_MASK: u32 = 0x00FF_FFFF;

#[cfg(all(target_os = "linux", target_env = "musl"))]
type Req = libc::c_int;
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
type Req = libc::c_ulong;

#[cfg(target_os = "linux")]
fn ioctl_u32(fd: libc::c_int, req: libc::c_ulong, val: u32) -> io::Result<()> {
    let v = val;
    let r = unsafe { libc::ioctl(fd, req as Req, &v as *const u32) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Transmit on one emitter. `durations` alternate mark and space in
/// MICROSECONDS, starting with a mark.
///
/// The kernel rejects an even count, so the trailing space is dropped rather
/// than surfacing as a bare EINVAL from `write`.
#[cfg(target_os = "linux")]
pub fn send(dev: &Path, carrier_hz: u32, durations: &[u32]) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    if durations.is_empty() {
        return Err(io::Error::other("no burst durations"));
    }
    let f = std::fs::OpenOptions::new().read(true).write(true).open(dev)?;
    let fd = f.as_raw_fd();
    ioctl_u32(fd, LIRC_SET_SEND_CARRIER, carrier_hz)?;

    let n = if durations.len() % 2 == 0 { durations.len() - 1 } else { durations.len() };
    let mut buf = Vec::with_capacity(n * 4);
    for d in &durations[..n] {
        buf.extend_from_slice(&d.to_ne_bytes());
    }
    let w = unsafe { libc::write(fd, buf.as_ptr() as *const _, buf.len()) };
    if w < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Open the receiver in mode2, non-blocking.
#[cfg(target_os = "linux")]
pub fn open_rx(dev: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(dev)?;
    // Not fatal: a raw-IR device is already in mode2.
    let _ = ioctl_u32(f.as_raw_fd(), LIRC_SET_REC_MODE, LIRC_MODE_MODE2);
    Ok(f)
}

#[cfg(not(target_os = "linux"))]
pub fn send(_dev: &Path, _carrier_hz: u32, _durations: &[u32]) -> io::Result<()> {
    Err(io::Error::other("lirc is only available on Linux"))
}

#[cfg(not(target_os = "linux"))]
pub fn open_rx(_dev: &Path) -> io::Result<std::fs::File> {
    Err(io::Error::other("lirc is only available on Linux"))
}
