//! HC-250 (TI OMAP3630): what the SD install and the way back are made of.
//!
//! The work runs on the box, in scripts the board's image ships
//! (`board/hc250/`); the flasher sequences them over SSH:
//!
//! * stock: `ohc-kexec.sh` + `ohc_kexec.ko` start openHC in RAM (stock has no
//!   kexec); `ohc-boot-env.sh` writes U-Boot's env with stock's own fw_setenv and
//!   prints stock's IO firmware tree.
//! * openHC: `ohc-sd-install` partitions the SD, writes the image and arms the
//!   EEPROM try count; `restore stock` puts Control4's IO firmware back and
//!   requests U-Boot's factory restore.
//!
//! Linux never writes the NAND: mainline's BCH4 OOB layout is not Control4's, so
//! a page written from Linux reads back as uncorrectable in U-Boot.

/// The flasher's image: kernel + appended DTB + initramfs, one legacy uImage.
pub const IMAGE_FILE: &str = "openhc-hc250-kernel.img";
/// What the image calls itself in the uImage header.
pub const IMAGE_NAME: &str = "hc250";
/// Stock-side kit, shipped in the same release bundle.
pub const KEXEC_KO: &str = "ohc_kexec.ko";
pub const KEXEC_SH: &str = "ohc-kexec.sh";
pub const BOOT_ENV_SH: &str = "ohc-boot-env.sh";

/// Where the image goes and what the SD partitions are called.
pub const SD_IMAGE: &str = "openhc.img";
pub const BOOT_LABEL: &str = "OHCBOOT";
pub const DATA_LABEL: &str = "ohcdata";

/// EEPROM byte U-Boot's `bootohc` counts tries in, and the restore request.
pub const TRY_BYTE: u8 = 6;
pub const REQUEST_BYTE: u8 = 7;
/// Failed openHC boots before U-Boot boots stock.
pub const MAX_TRIES: u32 = 3;

/// `ohc_kexec.ko` is built against Control4's 2.6.32 headers and 2.10.6's export
/// CRCs, and loads into every Control4 2.6.32 kernel: with CRCs present the
/// loader skips vermagic's release, and the kernel functions it calls it finds
/// through kallsyms at run time. Proven on 2.6.32-51.48 (2.10.6) and 2.6.32-47.41
/// (2.5.3, the factory-restore image).
pub const KEXEC_KERNEL: &str = "2.6.32-";

/// The uImage is loaded at 0x82000000 and run in place; below 0x8a000000, where
/// `bootohc` keeps its scratch byte.
pub const MAX_IMAGE: usize = 0x8a00_0000 - 0x8200_0040;

/// Check a uImage before it goes near the box: the magic, that it is an HC-250
/// image, and that U-Boot can load it.
pub fn check_image(img: &[u8]) -> Result<(), String> {
    if img.len() < 64 || img[..4] != [0x27, 0x05, 0x19, 0x56] {
        return Err("not a U-Boot uImage".into());
    }
    let name = String::from_utf8_lossy(&img[32..64]);
    let name = name.trim_end_matches('\0');
    if !name.contains(IMAGE_NAME) {
        return Err(format!("uImage is \"{name}\", not an HC-250 image"));
    }
    if img.len() > MAX_IMAGE {
        return Err(format!("image is {} bytes; U-Boot loads at most {MAX_IMAGE}", img.len()));
    }
    Ok(())
}

/// Can this stock kexec openHC? Any Control4 2.6.32 kernel (every HC-250 OS).
pub fn kexec_supported(uname_r: &str) -> Result<(), String> {
    let r = uname_r.trim();
    if r.starts_with(KEXEC_KERNEL) {
        Ok(())
    } else {
        Err(format!("stock runs kernel {r}; ohc_kexec.ko is for Control4's 2.6.32 kernels"))
    }
}

/// openHC booted by U-Boot from the SD install (not a RAM boot).
pub fn booted_from_sd(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "ohc.boot=sd")
}

/// `ohc-boot-env.sh status` -> the env's `bootmode`.
pub fn bootmode(status: &str) -> Option<&str> {
    status.lines().find_map(|l| l.trim().strip_prefix("bootmode=")).map(str::trim)
}

/// `restore status` -> its `state:` line.
pub fn restore_state(status: &str) -> Option<&str> {
    status.lines().find_map(|l| l.trim().strip_prefix("state:")).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uimage(name: &str, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len.max(64)];
        v[..4].copy_from_slice(&[0x27, 0x05, 0x19, 0x56]);
        v[32..32 + name.len()].copy_from_slice(name.as_bytes());
        v
    }

    #[test]
    fn images_are_checked() {
        assert_eq!(check_image(&uimage("openHC hc250", 100)), Ok(()));
        assert!(check_image(&uimage("openHC ioxv1", 100)).is_err(), "another board's image");
        assert!(check_image(b"not a uimage, definitely not one, long enough to look real........").is_err());
        assert!(check_image(&uimage("openHC hc250", MAX_IMAGE + 1)).is_err(), "too big for U-Boot");
    }

    #[test]
    fn kexec_from_any_control4_2_6_32_kernel() {
        assert!(kexec_supported("2.6.32-51.48\n").is_ok(), "2.10.6");
        assert!(kexec_supported("2.6.32-47.41").is_ok(), "2.5.3, the factory-restore image");
        assert!(kexec_supported("7.1.8").is_err(), "not stock");
    }

    #[test]
    fn an_sd_boot_says_so_on_its_command_line() {
        assert!(booted_from_sd("mem=504M eth=00:0f:ff:1a:a0:b1 console=ttyS0,115200n8 panic=10 ohc.boot=sd"));
        assert!(!booted_from_sd("console=ttyS0,115200n8 eth=00:0f:ff:1a:a0:b1 ohc.ramboot=1"));
    }

    #[test]
    fn status_lines_parse() {
        let env = "bootmode=ohc\nrestoresystem=run restorekern; ...\nbootohc=i2c dev 0; ...\n";
        assert_eq!(bootmode(env), Some("ohc"));
        let r = "EEPROM: try 0x00, restore request 0xff\nstate: openHC (installed; restorable to stock)\n";
        assert_eq!(restore_state(r), Some("openHC (installed; restorable to stock)"));
        assert_eq!(bootmode("nothing"), None);
    }

    #[test]
    fn the_partition_labels_match_the_box_scripts() {
        // board/hc250/rootfs-overlay/usr/sbin/ohc-sd-install and
        // packages/restore/src/hc250.rs use the same names.
        let script = include_str!("../../../../board/hc250/rootfs-overlay/usr/sbin/ohc-sd-install");
        assert!(script.contains(&format!("mkfs.vfat -n {BOOT_LABEL}")));
        assert!(script.contains(&format!("-L {DATA_LABEL}")));
        assert!(script.contains(&format!("IMG={SD_IMAGE}")));
        assert!(script.contains(&format!("TRY={TRY_BYTE}")));
        let env = include_str!("../../../../board/hc250/stock-kexec/ohc-boot-env.sh");
        assert!(env.contains(&format!("i2c read 50 {TRY_BYTE} 1")));
        assert!(env.contains(&format!("i2c read 50 {REQUEST_BYTE} 1")));
        assert!(env.contains(&format!("-ge {MAX_TRIES}")));
    }
}
