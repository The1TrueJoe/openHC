//! Put Control4's own NCP firmware back: the return-to-stock half of ncp.rs.
//!
//! openHC reflashes the radio to its own EmberZNet at boot, and a Control4
//! controller handed back to stock should look like openHC never ran on it, so
//! every return to stock (restore, the flasher) runs `--restore-stock` first.
//! Control4 would put its image back by itself once its `zap` service runs (proven
//! on an HC-800), but only on a box that is made a Zigbee server again.
//!
//! The image is not something openHC ships. Each board's recovery archive (the
//! factory rootfs Control4's own restore unpacks) carries it, so it is taken from
//! there: `OHC_ZIGBEE_STOCK_ARCHIVE` names the partition and the archive on it,
//! `OHC_ZIGBEE_STOCK_MEMBER` the image inside (Control4 keeps it under
//! /control4/firmware/pro). The partition is only ever mounted read-only.
//!
//! A radio that runs something older than openHC's image was never reflashed by
//! openHC (it only ever moves a radio up to its own), so it is left alone.

use crate::ash::{self, Ash, NcpVersion};
use crate::ncp::{self, Config, Image};
use crate::{bootloader, set_baud};
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_MEMBER: &str = "./control4/firmware/pro/em357-uart-*.ebl";
const WORK: &str = "/run/ohc-zigbee-stock";

/// Bring the radio back to Control4's firmware. Ok carries what happened, for
/// the log; Err means the radio is not on Control4's firmware and could not be
/// put there.
pub fn restore(fd: RawFd, cfg: &Config, archive: Option<&str>, member: &str) -> Result<String, String> {
    let ours = cfg.image.as_ref().map(|i| i.stack);
    let found = ncp::find_app(fd, cfg);
    if let Some((b, v)) = &found {
        if ours.is_some_and(|o| v.stack < o) {
            return Ok(format!("the NCP runs {} at {b} baud, not openHC's image — nothing to put back", describe(v)));
        }
    } else {
        set_baud(fd, cfg.btl_baud);
        if !bootloader::at_prompt(fd) {
            return Ok("no NCP app or bootloader answers — the radio is left as it is".into());
        }
    }
    let now = found.as_ref().map_or("no valid app (its bootloader answers)".to_string(), |(_, v)| describe(v));
    let archive = archive.ok_or_else(|| {
        format!("the NCP has {now}, and this board names no OHC_ZIGBEE_STOCK_ARCHIVE to take Control4's image from")
    })?;
    let (src, data) = extract(archive, member)?;
    println!("zigbee: putting Control4's image back over {now}: {src} ({} bytes)", data.len());
    let img = Image { path: src, data, stack: 0 };

    match &found {
        Some((b, _)) => {
            set_baud(fd, *b);
            let mut a = Ash::new(fd);
            if !(a.reset() && a.version().is_some()) {
                return Err("lost the NCP before the flash; its firmware is unchanged".into());
            }
            ncp::flash(fd, &img, cfg.btl_baud, Some(&mut a))?;
        }
        None => ncp::flash(fd, &img, cfg.btl_baud, None)?,
    }
    match ncp::find_app(fd, cfg) {
        Some((b, v)) if ours.map_or(true, |o| v.stack < o) => {
            let _ = std::fs::remove_file(ncp::STATUS);
            Ok(format!("Control4's firmware is back: {} at {b} baud", describe(&v)))
        }
        Some((b, v)) => Err(format!("flashed, but the NCP still answers {} at {b} baud", describe(&v))),
        None => Err("flashed, but no NCP app answers".into()),
    }
}

fn describe(v: &NcpVersion) -> String {
    format!("EmberZNet {} (EZSP v{})", ash::stack_str(v.stack), v.protocol)
}

/// `<device>:<archive path on it>`, e.g. `/dev/sda2:/restore_images/root-fs-sherman.tgz`.
fn parse_archive(spec: &str) -> Result<(&str, &str), String> {
    match spec.split_once(':') {
        Some((dev, path)) if dev.starts_with("/dev/") && path.starts_with('/') => Ok((dev, path)),
        _ => Err(format!("OHC_ZIGBEE_STOCK_ARCHIVE={spec:?}: expected /dev/<partition>:/<archive>")),
    }
}

/// The command that streams an archive out as a plain tar, by its extension.
fn decompressor(path: &str) -> Result<&'static str, String> {
    if path.ends_with(".tar.xz") || path.ends_with(".txz") {
        Ok("unxz -c")
    } else if path.ends_with(".tgz") || path.ends_with(".tar.gz") {
        Ok("gunzip -c")
    } else if path.ends_with(".tar") {
        Ok("cat")
    } else {
        Err(format!("{path}: not a .tar, .tgz or .tar.xz"))
    }
}

/// Both spellings of a member, since archives differ on the leading "./".
fn member_patterns(member: &str) -> [String; 2] {
    let bare = member.trim_start_matches("./");
    [format!("./{bare}"), bare.to_string()]
}

/// Mounts the partition read-only for as long as it lives.
struct Mount(PathBuf);

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.0).status();
    }
}

/// Unpack the one image matching `member` from the archive, read-only.
fn extract(spec: &str, member: &str) -> Result<(String, Vec<u8>), String> {
    let (dev, path) = parse_archive(spec)?;
    let dec = decompressor(path)?;
    let pats = member_patterns(member);
    if [spec, member].iter().any(|s| s.contains('\'')) {
        return Err("a quote in the archive or member name; not passing it through a shell".into());
    }
    let mnt = Path::new(WORK).join("mnt");
    let out = Path::new(WORK).join("x");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&mnt).and_then(|_| std::fs::create_dir_all(&out)).map_err(|e| format!("{WORK}: {e}"))?;
    let ok = Command::new("mount").args(["-o", "ro", dev]).arg(&mnt).status().map(|s| s.success());
    if !matches!(ok, Ok(true)) {
        return Err(format!("cannot mount {dev} read-only"));
    }
    let guard = Mount(mnt.clone());
    let arch = mnt.join(path.trim_start_matches('/'));
    if !arch.exists() {
        return Err(format!("{dev} has no {path}"));
    }
    println!("zigbee: looking for {member} in {dev}:{path}");
    // busybox tar matches the names with fnmatch, but exits 1 for whichever
    // spelling did not occur, so the result is judged by what landed.
    let sh = format!(
        "{dec} '{}' | tar -x -f - -C '{}' '{}' '{}' 2>/dev/null; true",
        arch.display(),
        out.display(),
        pats[0],
        pats[1]
    );
    let _ = Command::new("sh").args(["-c", &sh]).status();
    drop(guard);
    let mut found = Vec::new();
    collect_ebl(&out, &mut found);
    let r = match found.as_slice() {
        [one] => std::fs::read(one)
            .map(|d| (format!("{dev}:{path}:{}", one.strip_prefix(&out).unwrap_or(one).display()), d))
            .map_err(|e| format!("{}: {e}", one.display())),
        [] => Err(format!("no {member} in {dev}:{path}")),
        many => Err(format!(
            "{} images match {member} in {dev}:{path}; set OHC_ZIGBEE_STOCK_MEMBER to one of: {}",
            many.len(),
            many.iter().map(|p| p.strip_prefix(&out).unwrap_or(p).display().to_string()).collect::<Vec<_>>().join(", ")
        )),
    };
    let _ = std::fs::remove_dir_all(WORK);
    r
}

fn collect_ebl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_ebl(&p, out);
        } else if p.extension().is_some_and(|x| x == "ebl") {
            out.push(p);
        }
    }
    out.sort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_spec() {
        assert_eq!(
            parse_archive("/dev/sda2:/restore_images/root-fs-sherman.tgz"),
            Ok(("/dev/sda2", "/restore_images/root-fs-sherman.tgz"))
        );
        assert!(parse_archive("sda2:/x.tgz").is_err());
        assert!(parse_archive("/dev/sda2").is_err());
    }

    #[test]
    fn decompressor_by_extension() {
        assert_eq!(decompressor("/recfs.tar.xz"), Ok("unxz -c"));
        assert_eq!(decompressor("/restore_images/root-fs-sherman.tgz"), Ok("gunzip -c"));
        assert_eq!(decompressor("/a.tar"), Ok("cat"));
        assert!(decompressor("/a.zip").is_err());
    }

    #[test]
    fn both_member_spellings() {
        assert_eq!(member_patterns(DEFAULT_MEMBER), [DEFAULT_MEMBER.to_string(), DEFAULT_MEMBER[2..].to_string()]);
        assert_eq!(member_patterns("control4/x.ebl"), ["./control4/x.ebl".to_string(), "control4/x.ebl".to_string()]);
    }
}
