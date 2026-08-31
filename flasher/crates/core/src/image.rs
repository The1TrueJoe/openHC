//! Release-artifact validation and the CEFDK container builder.
//!
//! An openHC release is a set of files (a directory or an unpacked zip): a
//! `bzImage`, an initramfs `rootfs.cpio.gz`, and an ext2 `rootfs.ext2` for the
//! EA boards; a `zImage` + DTB + `boot.scr` for the CA-1. This module knows
//! only how to recognise and sanity-check them; reading the filesystem or a zip
//! is the caller's job, so `core` stays I/O-free.

use crate::cefdk;

/// `bootlinux` copies the protected-mode kernel to 0x100000 while CEFDK's own
/// loader sits at ~0x813000. A bzImage past this window clobbers the loader
/// mid-copy and the board does not boot — a failure that costs an ID-button
/// recovery, so it is checked here rather than discovered on hardware.
pub const BOOTLINUX_WINDOW: u64 = 0x813000 - 0x100000; // 7,417,856

/// Is this a Linux bzImage? Checks the boot-sector magic and the "HdrS" tag.
pub fn is_bzimage(head: &[u8]) -> bool {
    head.len() > 0x206
        && head[0x1fe] == 0x55
        && head[0x1ff] == 0xaa
        && &head[0x202..0x206] == b"HdrS"
}

/// Everything wrong with a would-be EA release, in the order a user should see
/// it. Empty means good to flash.
pub fn ea_problems(kernel_head: Option<&[u8]>, kernel_len: u64, has_rootfs: bool, need_rootfs: bool) -> Vec<String> {
    let mut out = vec![];
    match kernel_head {
        None => out.push("no bzImage in the release".into()),
        Some(head) => {
            if kernel_len > BOOTLINUX_WINDOW {
                out.push(format!(
                    "bzImage is {kernel_len} B, over CEFDK's {BOOTLINUX_WINDOW} B bootlinux \
                     window — it would overwrite the loader mid-copy and not boot"
                ));
            }
            if !is_bzimage(head) {
                out.push("kernel does not look like a bzImage (no 0x55aa/HdrS magic)".into());
            }
        }
    }
    if need_rootfs && !has_rootfs {
        out.push("no rootfs.ext2 in the release".into());
    }
    out
}

pub fn headroom(kernel_len: u64) -> i64 {
    BOOTLINUX_WINDOW as i64 - kernel_len as i64
}

/// Wrap a bzImage in a CEFDK container.
///
/// The 0x580-byte header is GENERATED, not copied from a vendor image: the
/// stock header carries RSA signature material, and shipping that in an
/// MIT-licensed repo would redistribute Control4 binary content. Only a few
/// fields are read on the bootlinux path, and they are set here. A caller that
/// has legitimately extracted a header from its own unit may pass one.
pub fn container(kernel: &[u8], header: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let hdr: Vec<u8> = match header {
        Some(h) => {
            if h.len() != cefdk::EMMC_HEADER_LEN {
                return Err(format!("header must be {} bytes", cefdk::EMMC_HEADER_LEN));
            }
            h.to_vec()
        }
        None => {
            let mut h = vec![0u8; cefdk::EMMC_HEADER_LEN];
            h[0x10..0x14].copy_from_slice(&0x8086u32.to_le_bytes()); // Intel vendor
            h[0x28..0x2c].copy_from_slice(&(cefdk::EMMC_HEADER_LEN as u32).to_le_bytes()); // payload off
            h[0x2c..0x30].copy_from_slice(&(kernel.len() as u32).to_le_bytes()); // payload len
            h
        }
    };
    let mut out = hdr;
    out.extend_from_slice(kernel);
    Ok(out)
}

/// The device wants the total container length as a big-endian u32 at
/// `EMMC_SIZE_OFF`.
pub fn size_word(total_len: usize) -> [u8; 4] {
    (total_len as u32).to_be_bytes()
}
