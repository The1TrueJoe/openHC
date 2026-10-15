//! Finding external drives and mounting them.
//!
//! A drive is EXTERNAL when its sysfs path runs through a USB controller, or
//! through an ATA port the board names in `OHC_STORAGE_ATA` (the HC-800's eSATA
//! jack — its internal disk is on ata1 and holds Control4's factory restore,
//! which nothing here may ever touch). On top of that, a disk with any
//! partition mounted by someone else is left alone entirely.
//!
//! Each filesystem on an external disk (a partition, or the whole disk when it
//! has no partition table) is mounted at `/media/<label>`. What a partition
//! holds comes from udev (`udevadm info`: ID_FS_TYPE, ID_FS_LABEL,
//! ID_PART_ENTRY_TYPE), which every board runs — busybox's blkid reports no
//! type. EFI system partitions and anything not a data filesystem are skipped.
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

pub const MEDIA: &str = "/media";

/// GPT type of an EFI system partition: present on most installer sticks and
/// every Mac-formatted disk, never what anyone means by "the drive".
const ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
/// macOS's hidden HFS+/APFS helper partitions are also not shareable data.
const SUPPORTED: &[&str] = &["vfat", "exfat", "ntfs", "ntfs3", "ext4", "ext3", "ext2", "hfsplus"];

/// One mounted (or mountable) filesystem on an external drive.
#[derive(Serialize, Clone, Debug, PartialEq, utoipa::ToSchema)]
pub struct Volume {
    /// Stable id: the kernel name (`sdb2`).
    pub id: String,
    /// The share and mount name: the filesystem label, made safe, unique.
    pub name: String,
    pub label: String,
    pub fs: String,
    /// `usb` or `esata`.
    pub bus: String,
    /// Vendor + model of the drive it is on.
    pub drive: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_bytes: Option<u64>,
    /// Where it is mounted, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    pub read_only: bool,
}

/// A candidate found by `scan`, before mounting.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: String,
    pub dev: String,
    pub label: String,
    pub fs: String,
    pub bus: String,
    pub drive: String,
    pub size_bytes: u64,
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// `udevadm info --query=property` output → map.
pub fn parse_props(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn props(dev: &str) -> HashMap<String, String> {
    Command::new("udevadm")
        .args(["info", "--query=property", &format!("--name={dev}")])
        .output()
        .map(|o| parse_props(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Which external bus a disk's sysfs path runs through, if any.
pub fn bus_of(sys_path: &str, ata_ports: &[String]) -> Option<&'static str> {
    if sys_path.split('/').any(|c| c.starts_with("usb")) {
        return Some("usb");
    }
    let port = sys_path.split('/').find(|c| c.starts_with("ata") && c[3..].chars().all(|d| d.is_ascii_digit()))?;
    ata_ports.iter().any(|p| p == port).then_some("esata")
}

/// Devices mounted anywhere, from /proc/mounts: device → mount points.
pub fn mounts() -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    for l in read("/proc/mounts").lines() {
        let mut f = l.split_whitespace();
        if let (Some(dev), Some(at)) = (f.next(), f.next()) {
            m.entry(dev.to_string()).or_default().push(unescape(at));
        }
    }
    m
}

/// /proc/mounts escapes spaces and friends as octal (`\040`).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every shareable filesystem on an external disk right now.
pub fn scan(ata_ports: &[String]) -> Vec<Found> {
    let mounted = mounts();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/block") else { return out };
    let mut disks: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    disks.sort();
    for disk in disks {
        if !disk.starts_with("sd") {
            continue; // USB and SATA disks are all SCSI disks
        }
        let sys = std::fs::canonicalize(format!("/sys/block/{disk}")).map(|p| p.display().to_string()).unwrap_or_default();
        let Some(bus) = bus_of(&sys, ata_ports) else { continue };
        // The partitions, or the bare disk when it has none.
        let mut parts: Vec<String> = std::fs::read_dir(format!("/sys/block/{disk}"))
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&disk)).collect())
            .unwrap_or_default();
        parts.sort();
        if parts.is_empty() {
            parts.push(disk.clone());
        }
        // Someone else's mount anywhere on this disk: hands off the whole disk.
        let foreign = std::iter::once(&disk).chain(parts.iter()).any(|p| {
            mounted.get(&format!("/dev/{p}")).is_some_and(|ats| ats.iter().any(|a| !a.starts_with(&format!("{MEDIA}/"))))
        });
        if foreign {
            continue;
        }
        let drive = [read(format!("/sys/block/{disk}/device/vendor")), read(format!("/sys/block/{disk}/device/model"))]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        for p in parts {
            let dev = format!("/dev/{p}");
            let pr = props(&dev);
            let fs = pr.get("ID_FS_TYPE").cloned().unwrap_or_default();
            if !SUPPORTED.contains(&fs.as_str()) || pr.get("ID_PART_ENTRY_TYPE").is_some_and(|t| t.eq_ignore_ascii_case(ESP)) {
                continue;
            }
            let sectors: u64 = read(format!("/sys/class/block/{p}/size")).parse().unwrap_or(0);
            out.push(Found {
                id: p.clone(),
                dev,
                label: pr.get("ID_FS_LABEL").cloned().unwrap_or_default(),
                fs,
                bus: bus.into(),
                drive: drive.clone(),
                size_bytes: sectors * 512,
            });
        }
    }
    out
}

/// A share/mount name from a label: letters, digits, space, `-`, `_`, `.`;
/// falls back to the device; made unique against `taken`.
pub fn share_name(label: &str, id: &str, taken: &[String]) -> String {
    let clean: String = label
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(32)
        .collect();
    let base = if clean.is_empty() { format!("USB {id}") } else { clean };
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{base} {n}");
        n += 1;
    }
    name
}

/// Mount options per filesystem: FAT/exFAT/NTFS have no owners, so everything
/// is root's and writable by the share (which forces root).
pub fn options(fs: &str, ro: bool) -> String {
    let base = if ro { "ro,noatime" } else { "rw,noatime" };
    match fs {
        "vfat" => format!("{base},uid=0,gid=0,umask=0000,iocharset=utf8,shortname=mixed,flush"),
        "exfat" => format!("{base},uid=0,gid=0,umask=0000,iocharset=utf8"),
        "ntfs" | "ntfs3" => format!("{base},uid=0,gid=0,umask=0000,iocharset=utf8"),
        _ => base.to_string(),
    }
}

/// Mount `f` at /media/<name>; read-only if read-write is refused (a dirty
/// NTFS, a write-protect switch). Returns whether it ended up read-only.
pub fn mount(f: &Found, name: &str) -> Result<bool, String> {
    let at = format!("{MEDIA}/{name}");
    std::fs::create_dir_all(&at).map_err(|e| format!("mkdir {at}: {e}"))?;
    let fs = if f.fs == "ntfs" { "ntfs3" } else { f.fs.as_str() };
    for ro in [false, true] {
        let st = Command::new("mount").args(["-t", fs, "-o", &options(fs, ro), &f.dev, &at]).status();
        if matches!(st, Ok(s) if s.success()) {
            return Ok(ro);
        }
    }
    let _ = std::fs::remove_dir(&at);
    Err(format!("cannot mount {} ({}) at {at}", f.dev, f.fs))
}

/// Unmount and remove the mount point. A busy filesystem is detached lazily
/// (it is gone from the namespace now, and closes when its last user does) —
/// for a drive already pulled out there is nothing else to do anyway.
pub fn unmount(at: &str) {
    let st = Command::new("umount").arg(at).status();
    if !matches!(st, Ok(s) if s.success()) {
        let _ = Command::new("umount").args(["-l", at]).status();
    }
    let _ = std::fs::remove_dir(at);
}

/// Total and used bytes of a mounted filesystem (`df -k`, busybox's too).
pub fn usage(at: &str) -> Option<(u64, u64)> {
    let o = Command::new("df").args(["-k", at]).output().ok()?;
    let text = String::from_utf8_lossy(&o.stdout);
    let line = text.lines().nth(1)?;
    let f: Vec<&str> = line.split_whitespace().collect();
    Some((f.get(1)?.parse::<u64>().ok()? * 1024, f.get(2)?.parse::<u64>().ok()? * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_buses() {
        let ata = vec!["ata2".to_string()];
        assert_eq!(bus_of("/sys/devices/pci0000:00/0000:00:1d.7/usb2/2-3/2-3:1.0/host4/target4:0:0/4:0:0:0/block/sdb", &ata), Some("usb"));
        assert_eq!(bus_of("/sys/devices/pci0000:00/0000:00:1f.2/ata1/host0/target0:0:0/0:0:0:0/block/sda", &ata), None);
        assert_eq!(bus_of("/sys/devices/pci0000:00/0000:00:1f.2/ata2/host1/target1:0:0/1:0:0:0/block/sdc", &ata), Some("esata"));
        assert_eq!(bus_of("/sys/devices/platform/soc/2100000.mmc/mmc_host/mmc0/mmc0:0001/block/mmcblk0", &ata), None);
    }
    #[test]
    fn names_are_safe_and_unique() {
        assert_eq!(share_name("Sandisk", "sdb2", &[]), "Sandisk");
        assert_eq!(share_name("My/Drive:*", "sdb1", &[]), "MyDrive");
        assert_eq!(share_name("", "sdc1", &[]), "USB sdc1");
        assert_eq!(share_name("..hidden", "sdc1", &[]), "hidden");
        assert_eq!(share_name("Music", "sdc1", &["music".into()]), "Music 2");
    }
    #[test]
    fn udev_props_and_proc_mounts() {
        let p = parse_props("ID_FS_TYPE=exfat\nID_FS_LABEL=Sandisk\nID_PART_ENTRY_TYPE=ebd0a0a2-b9e5-4433-87c0-68b6b72699c7\n");
        assert_eq!(p.get("ID_FS_TYPE").map(String::as_str), Some("exfat"));
        assert_eq!(unescape("/media/My\\040Drive"), "/media/My Drive");
    }
    #[test]
    fn fat_is_owned_by_root_and_writable() {
        assert!(options("exfat", false).starts_with("rw,noatime,uid=0"));
        assert!(options("vfat", true).starts_with("ro,"));
        assert_eq!(options("ext4", false), "rw,noatime");
    }
}
