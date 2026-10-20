//! Putting a downloaded image on THIS box — one installer per board.
//!
//! The same writes the flasher does over SSH, done locally. Only the HC-800 has
//! one so far; the others check and download, and their install says to use
//! the flasher until theirs lands (each board stores its kernel differently:
//! the EAs in the MFH/eMMC, the CA-1 behind boot.scr, the IO Extender in NAND).
use ohc_flash_core::hc800 as hc;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn supported(board: &str) -> bool {
    board == "hc800"
}

pub fn install(board: &str, bundle: &Path) -> Result<(), String> {
    match board {
        "hc800" => hc800(bundle),
        other => Err(format!("no on-box installer for {other} yet — install this release with ohc-flash")),
    }
}

/// The first of `names` present in the bundle.
fn read_one(z: &mut zip::ZipArchive<File>, names: &[&str]) -> Result<Vec<u8>, String> {
    for n in names {
        if let Ok(mut f) = z.by_name(n) {
            let mut b = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut b).map_err(|e| format!("read {n}: {e}"))?;
            return Ok(b);
        }
    }
    Err(format!("the bundle has none of {}", names.join(", ")))
}

/// A bzImage carries "HdrS" at 0x202 (the x86 boot protocol header).
pub fn looks_like_bzimage(b: &[u8]) -> bool {
    b.len() > 0x206 && &b[0x202..0x206] == b"HdrS"
}
pub fn looks_like_gzip(b: &[u8]) -> bool {
    b.len() > 2 && b[0] == 0x1f && b[1] == 0x8b
}

/// The HC-800: replace the kernel and initramfs on the kernel partition (sda3),
/// which is all the persistent install's GRUB entry boots. Nothing else on the
/// disk is touched — not the bootloader partition, not the factory restore
/// partition (sda2), not the MBR. Each file is written beside the old one and
/// renamed over it, so a power cut leaves either image whole.
fn hc800(bundle: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(File::open(bundle).map_err(|e| format!("{}: {e}", bundle.display()))?)
        .map_err(|e| format!("not a zip: {e}"))?;
    let kernel = read_one(&mut z, &["openhc-hc800-kernel.img", "bzImage"])?;
    let initrd = read_one(&mut z, &["openhc-initrd.gz", "rootfs.cpio.gz"])?;
    if !looks_like_bzimage(&kernel) {
        return Err("the bundle's kernel is not a bzImage".into());
    }
    if !looks_like_gzip(&initrd) {
        return Err("the bundle's initramfs is not gzip".into());
    }
    let mnt = PathBuf::from("/run/ohc/update/kernel");
    std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
    let st = Command::new("mount").arg(hc::KERNEL_PART).arg(&mnt).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("cannot mount {}", hc::KERNEL_PART));
    }
    let r = (|| {
        for (file, data) in [(hc::KERNEL_FILE, &kernel), (hc::INITRD_FILE, &initrd)] {
            let dst = mnt.join(file.trim_start_matches('/'));
            let tmp = dst.with_extension("new");
            let mut f = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
            drop(f);
            std::fs::rename(&tmp, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            let got = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            if got != data.len() as u64 {
                return Err(format!("{file}: wrote {got} bytes, expected {}", data.len()));
            }
        }
        Ok(())
    })();
    let _ = Command::new("sync").status();
    let _ = Command::new("umount").arg(&mnt).status();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognises_the_images() {
        let mut k = vec![0u8; 0x300];
        k[0x202..0x206].copy_from_slice(b"HdrS");
        assert!(looks_like_bzimage(&k));
        assert!(!looks_like_bzimage(&[0u8; 0x300]));
        assert!(looks_like_gzip(&[0x1f, 0x8b, 8]));
        assert!(!looks_like_gzip(b"PK"));
        assert!(supported("hc800") && !supported("ca1"));
    }
}
