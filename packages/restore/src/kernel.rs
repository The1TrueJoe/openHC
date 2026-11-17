//! The EA's stock kernel slot: put Control4's kernel back where CEFDK's own
//! (non-autoscript) boot reads it.
//!
//! Control4's kernel package (p2's recovery_kernel.deb) installs its kernel
//! container at byte 1024 of the eMMC and the container's byte size, big-endian,
//! in the 4 bytes at 512, then checks the written copy against the md5 listed in
//! its `md5sums`. This does the same from the same package: nothing of the
//! kernel comes from openHC. An openHC install may have put its own kernel in
//! that slot; with the fuse blown, CEFDK's verifying boot would refuse it.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

use md5::{Digest, Md5};

const EMMC: &str = "/dev/mmcblk0";
const KERNEL_OFF: u64 = 1024;
const SIZE_OFF: u64 = 512;
/// The kernel's path inside the package's data and in its md5sums.
const KERNEL_MEMBER: &str = "tmp/bzImage";
const WRITE_TRIES: usize = 3;

/// One member of an `ar` archive (a .deb is one), by name.
fn ar_member(deb: &[u8], want: &str) -> Option<Vec<u8>> {
    let mut off = deb.strip_prefix(b"!<arch>\n").map(|_| 8)?;
    while off + 60 <= deb.len() {
        let hdr = &deb[off..off + 60];
        let name = std::str::from_utf8(&hdr[..16]).ok()?.trim_end().trim_end_matches('/');
        let size: usize = std::str::from_utf8(&hdr[48..58]).ok()?.trim().parse().ok()?;
        let start = off + 60;
        let data = deb.get(start..start + size)?;
        if name == want {
            return Some(data.to_vec());
        }
        off = start + size + (size & 1);
    }
    None
}

/// Unpack a `.tar.gz` member into `dir` with the box's gunzip and tar.
fn untar_gz(data: &[u8], dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tgz = dir.join("m.tar.gz");
    std::fs::write(&tgz, data)?;
    crate::run("gunzip", &["-f", &tgz.to_string_lossy()])?;
    let tar = dir.join("m.tar");
    let r = crate::run("tar", &["-xf", &tar.to_string_lossy(), "-C", &dir.to_string_lossy()]);
    let _ = std::fs::remove_file(&tar);
    r
}

/// The md5 the package lists for `member`, from its md5sums.
fn listed_md5(md5sums: &str, member: &str) -> Option<String> {
    md5sums.lines().find_map(|l| {
        let (sum, path) = l.split_once(char::is_whitespace)?;
        (path.trim().trim_start_matches("./") == member).then(|| sum.to_ascii_lowercase())
    })
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// The stock kernel container and its listed md5, from p2's recovery_kernel.deb.
pub fn stock_container(p2: &Path) -> io::Result<Vec<u8>> {
    let deb = std::fs::read(p2.join("recovery_kernel.deb"))?;
    let missing = |m: &str| io::Error::other(format!("recovery_kernel.deb has no {m}"));
    let work = std::env::temp_dir().join("ohc-stock-kernel");
    let _ = std::fs::remove_dir_all(&work);
    untar_gz(&ar_member(&deb, "control.tar.gz").ok_or_else(|| missing("control.tar.gz"))?, &work.join("control"))?;
    untar_gz(&ar_member(&deb, "data.tar.gz").ok_or_else(|| missing("data.tar.gz"))?, &work.join("data"))?;
    let sums = std::fs::read_to_string(work.join("control/md5sums"))?;
    let want = listed_md5(&sums, KERNEL_MEMBER).ok_or_else(|| missing("md5sums entry for the kernel"))?;
    let kernel = std::fs::read(work.join("data").join(KERNEL_MEMBER))?;
    let _ = std::fs::remove_dir_all(&work);
    let got = md5_hex(&kernel);
    if got != want {
        return Err(io::Error::other(format!("stock kernel md5 {got} is not the package's {want}")));
    }
    if crate::bzimage_offset(&kernel).is_none() {
        return Err(io::Error::other("the stock kernel container holds no bzImage"));
    }
    Ok(kernel)
}

fn read_at(f: &File, off: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    f.read_exact_at(&mut buf, off)?;
    Ok(buf)
}

/// True when the slot already holds exactly `kernel` with its size.
fn slot_matches(f: &File, kernel: &[u8]) -> io::Result<bool> {
    let size = (kernel.len() as u32).to_be_bytes();
    Ok(read_at(f, SIZE_OFF, 4)? == size && md5_hex(&read_at(f, KERNEL_OFF, kernel.len())?) == md5_hex(kernel))
}

/// Write Control4's kernel and its size into the slot and verify both by
/// read-back. The kernel goes first, so a failed write never leaves a size that
/// points past a short kernel.
pub fn install_stock_kernel(p2: &Path) -> io::Result<()> {
    let kernel = stock_container(p2)?;
    let mut f = OpenOptions::new().read(true).write(true).open(EMMC)?;
    if slot_matches(&f, &kernel)? {
        println!("kernel: Control4's kernel is already in the stock slot");
        return Ok(());
    }
    for attempt in 1..=WRITE_TRIES {
        f.seek(SeekFrom::Start(KERNEL_OFF))?;
        f.write_all(&kernel)?;
        f.sync_all()?;
        if md5_hex(&read_at(&f, KERNEL_OFF, kernel.len())?) != md5_hex(&kernel) {
            eprintln!("kernel: read-back mismatch, attempt {attempt}");
            continue;
        }
        f.write_all_at(&(kernel.len() as u32).to_be_bytes(), SIZE_OFF)?;
        f.sync_all()?;
        if slot_matches(&f, &kernel)? {
            println!("kernel: Control4's kernel ({} bytes) written to the stock slot and verified", kernel.len());
            return Ok(());
        }
        eprintln!("kernel: size read-back mismatch, attempt {attempt}");
    }
    Err(io::Error::other("could not write a verified stock kernel to the slot"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut v = b"!<arch>\n".to_vec();
        for (name, data) in members {
            let hdr = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", format!("{name}/"), 0, 0, 0, 100644, data.len());
            v.extend_from_slice(hdr.as_bytes());
            v.extend_from_slice(data);
            if data.len() % 2 == 1 {
                v.push(b'\n');
            }
        }
        v
    }

    #[test]
    fn ar_members_by_name() {
        let a = ar(&[("debian-binary", b"2.0\n"), ("control.tar.gz", b"abc"), ("data.tar.gz", b"0123")]);
        assert_eq!(ar_member(&a, "control.tar.gz").as_deref(), Some(&b"abc"[..]));
        assert_eq!(ar_member(&a, "data.tar.gz").as_deref(), Some(&b"0123"[..]));
        assert_eq!(ar_member(&a, "nope"), None);
        assert_eq!(ar_member(b"not an ar", "data.tar.gz"), None);
    }

    #[test]
    fn md5sums_lookup() {
        let sums = "00112233445566778899aabbccddeeff  tmp/other\n\
                    FFEEDDCCBBAA99887766554433221100  tmp/bzImage\n";
        assert_eq!(listed_md5(sums, "tmp/bzImage").as_deref(), Some("ffeeddccbbaa99887766554433221100"));
        assert_eq!(listed_md5("abc  ./tmp/bzImage\n", "tmp/bzImage").as_deref(), Some("abc"));
        assert_eq!(listed_md5(sums, "etc/x"), None);
    }

    #[test]
    fn md5_of_empty() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }
}
