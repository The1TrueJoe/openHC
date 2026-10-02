//! CA-1 return-to-factory: hand the box to Control4's own recovery, by command.
//!
//! openHC's install drops `boot.scr` (plus its own zImage and DTB) on the eMMC's
//! FAT partition, which stock `bootcmd` sources before its own zImage, and writes
//! its rootfs over p2. Only Control4's recovery re-images p2 (from p3), and stock
//! U-Boot already has it as a command: `factoryrestore` boots the recovery kernel
//! from SPI-NOR, the same thing the recessed button does.
//!
//! So a restore is: delete our three files, then make the NEXT boot run
//! `factoryrestore` once. The one-shot is built from plain `setenv` words, no
//! nested quoting, because a `bootcmd` U-Boot 2014's shell mis-parses would leave
//! a box with a locked console that the button cannot fix (it never resets the
//! environment):
//!
//!   ohc_stock_bootcmd = <the original bootcmd>
//!   ohc_restore_once  = setenv bootcmd run ohc_stock_bootcmd; saveenv; run factoryrestore
//!   bootcmd           = run ohc_restore_once
//!
//! After the recovery, `bootcmd` runs the original through one indirection.
//! Running the restore again on the restored stock box folds it back to the
//! literal original and deletes the two helper variables.

/// What openHC's install puts on the FAT partition (`board/ca1/post-image.sh`).
/// Stock's own zImage and DTB sit beside them and are never touched.
pub const OUR_FILES: &[&str] = &["boot.scr", "openhc-ca1-zImage", "c4-imx6sl-ca1.dtb"];
pub const FAT_PART: &str = "/dev/mmcblk1p1";

pub const STOCK_HOLD: &str = "ohc_stock_bootcmd";
pub const ONCE: &str = "ohc_restore_once";
pub const ONCE_SCRIPT: &str = "setenv bootcmd run ohc_stock_bootcmd; saveenv; run factoryrestore";
pub const ONCE_BOOTCMD: &str = "run ohc_restore_once";
pub const HELD_BOOTCMD: &str = "run ohc_stock_bootcmd";

/// The env partitions' names, as stock and as openHC's device tree call them.
pub const ENV_NAMES: &[(&str, &str)] = &[
    ("U-Boot Environment", "U-Boot Redundant Environment"),
    ("u-boot-env", "u-boot-env-redundant"),
];
/// One SPI-NOR sector: the env partitions are exactly one each.
pub const ENV_SECTOR: u32 = 0x1_0000;

/// zlib/IEEE CRC-32, the one U-Boot's environment uses.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// The environment's size and header length (4 = CRC, 5 = CRC + redundancy
/// flag), found the only reliable way: the length whose CRC matches. Nothing
/// here is guessed from a board header.
pub fn env_geometry(copy: &[u8]) -> Option<(u32, usize)> {
    if copy.len() < 8 {
        return None;
    }
    let crc = u32::from_le_bytes(copy[..4].try_into().ok()?);
    for size in [0x1000usize, 0x2000, 0x4000, 0x8000, 0x10000] {
        if size > copy.len() {
            break;
        }
        for hdr in [5usize, 4] {
            if crc32(&copy[hdr..size]) == crc {
                return Some((size as u32, hdr));
            }
        }
    }
    None
}

/// The two env mtd nodes, from `/proc/mtd`, whichever OS named them.
pub fn env_devices(proc_mtd: &str) -> Option<(String, String)> {
    let dev = |name: &str| {
        proc_mtd.lines().find_map(|l| {
            let (d, rest) = l.split_once(':')?;
            (rest.trim_end().ends_with(&format!("\"{name}\""))).then(|| format!("/dev/{d}"))
        })
    };
    ENV_NAMES.iter().find_map(|(a, b)| Some((dev(a)?, dev(b)?)))
}

/// A `fw_env.config` for the measured geometry.
pub fn fw_env_config(primary: &str, redundant: &str, size: u32) -> String {
    format!(
        "{primary} 0x0 {size:#x} {ENV_SECTOR:#x}\n{redundant} 0x0 {size:#x} {ENV_SECTOR:#x}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(size: usize, body: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; size];
        v[5..5 + body.len()].copy_from_slice(body);
        let c = crc32(&v[5..]);
        v[..4].copy_from_slice(&c.to_le_bytes());
        v[4] = 1;
        v.resize(ENV_SECTOR as usize, 0xff);
        v
    }

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_env_size_is_measured_not_guessed() {
        let e = env(0x2000, b"bootcmd=run foo\0\0");
        assert_eq!(env_geometry(&e), Some((0x2000, 5)));
        assert_eq!(env_geometry(&[0xffu8; 0x10000]), None, "an erased sector has no env");
    }

    #[test]
    fn env_devices_are_found_under_either_name() {
        let stock = "mtd2: 00010000 00010000 \"U-Boot Environment\"\n\
                     mtd5: 00010000 00010000 \"U-Boot Redundant Environment\"\n";
        let ohc = "mtd1: 00010000 00010000 \"u-boot-env\"\n\
                   mtd4: 00010000 00010000 \"u-boot-env-redundant\"\n";
        assert_eq!(env_devices(stock), Some(("/dev/mtd2".into(), "/dev/mtd5".into())));
        assert_eq!(env_devices(ohc), Some(("/dev/mtd1".into(), "/dev/mtd4".into())));
        assert_eq!(env_devices("mtd0: 01000000 00010000 \"SPI-NOR\"\n"), None);
    }

    #[test]
    fn the_one_shot_needs_no_quoting() {
        for s in [ONCE_SCRIPT, ONCE_BOOTCMD, HELD_BOOTCMD] {
            assert!(!s.contains('\'') && !s.contains('"'), "{s}");
        }
        assert!(ONCE_SCRIPT.ends_with("run factoryrestore"));
        assert!(ONCE_SCRIPT.starts_with(&format!("setenv bootcmd {HELD_BOOTCMD}")));
    }
}
