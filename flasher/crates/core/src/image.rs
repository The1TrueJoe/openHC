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

/// Everything wrong with a would-be HC-800 release. Empty means good to flash.
///
/// NOTE WHAT IS NOT CHECKED: there is no size window here, and that is the
/// point. The EA ceiling exists because CEFDK's `bootlinux` copies the kernel
/// into a fixed ~7 MB gap; the HC-800 boots from GRUB 0.97, which has no such
/// limit, and its kernel partition has ~165 MB free for a ~14 MB image. So the
/// only size question is whether the pair fits the partition, which the
/// installer asks the box directly rather than guessing from here.
///
/// An initramfs is REQUIRED rather than optional: openHC on this board runs
/// entirely from RAM, and a release with no `rootfs.cpio.gz` would boot a
/// kernel straight into a panic looking for a root it does not have.
pub fn hc_problems(kernel_head: Option<&[u8]>, kernel_len: u64, has_initrd: bool) -> Vec<String> {
    let mut out = vec![];
    match kernel_head {
        None => out.push("no bzImage in the release".into()),
        Some(head) => {
            if !is_bzimage(head) {
                out.push("kernel does not look like a bzImage (no 0x55aa/HdrS magic)".into());
            }
            if kernel_len == 0 {
                out.push("bzImage is empty".into());
            }
        }
    }
    if !has_initrd {
        out.push("no initramfs (rootfs.cpio.gz) — openHC runs from RAM on this board".into());
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
