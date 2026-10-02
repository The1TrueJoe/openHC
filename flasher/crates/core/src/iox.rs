//! IO Extender (ioxv1) NAND install: where openHC goes and how U-Boot boots it.
//!
//! The NAND is 512 MiB and stock Control4 partitions only the bottom half. openHC's
//! image (kernel + initramfs, ~17 MB) goes in a 32 MiB slot at the start of the
//! top half, so both stock systems, the recovery pair, the bootloaders and every
//! stock partition stay exactly as shipped.
//!
//! U-Boot's `bootcmd` becomes `run ohcboot`, which counts openHC boot attempts in
//! `ohc_try` and boots stock once it reaches [`MAX_TRIES`]. openHC's S95ohcboot
//! clears the count once its uplink is up, so a kernel that panics, hangs (U-Boot
//! arms the hardware watchdog; openHC feeds it) or never gets a network lands the
//! box back on stock with nobody touching it.
use crate::board::Running;

/// Start of the half of the NAND stock never partitioned.
pub const SLOT_OFF: u64 = 0x1000_0000;
/// One erase block.
pub const BLOCK: u64 = 0x2_0000;
/// 32 MiB: room for the image with margin, and far from the bad-block tables
/// in the last blocks of the chip.
pub const SLOT_BLOCKS: u64 = 256;
pub const SLOT_LEN: u64 = SLOT_BLOCKS * BLOCK;
/// openHC boot attempts before U-Boot falls back to stock.
pub const MAX_TRIES: u32 = 3;
/// What the Buildroot image calls itself in the uImage header.
pub const IMAGE_NAME: &str = "ioxv1";
pub const IMAGE_FILE: &str = "openhc-ioxv1-kernel.img";

/// U-Boot 1.2's shell. Three ordered tests step `ohc_try` up by exactly one
/// without nesting an `if` inside an `else`; the first test, at the limit, runs
/// the untouched stock boot and never returns. A slot with no valid image also
/// falls through to stock, because `nboot` or `bootm` fails and the script ends
/// on `run oldbootcmd`.
pub const OHCBOOT: &str = "if itest ${ohc_try} -ge 3; then run oldbootcmd; fi; \
if itest ${ohc_try} -eq 2; then setenv ohc_try 3; fi; \
if itest ${ohc_try} -eq 1; then setenv ohc_try 2; fi; \
if itest ${ohc_try} -eq 0; then setenv ohc_try 1; fi; \
saveenv; \
if nboot.e ${bootload} 0 ${ohc_addr}; then setenv bootargs mem=${memsize} ${ohc_bootargs}; \
if itest ${usewatchdogtimer} -eq 1; then c4sys watchdog c5986200 3; fi; bootm ${bootload}; fi; \
run oldbootcmd";

pub const BOOTARGS: &str = "console=ttyS0,115200n8 panic=10";
pub const OPENHC_BOOTCMD: &str = "run ohcboot";
/// Stock's own boot. `oldbootcmd` is the vendor's unmodified sequence (boot
/// counters, bank select, watchdog, bootm), kept in every unit's environment.
pub const STOCK_BOOTCMD: &str = "run oldbootcmd";

/// U-Boot's compiled-in default `bootcmd`, read out of the bootloader on the
/// unit: what a factory IO Extender boots with.
pub const FACTORY_BOOTCMD: &str = "if itest ${usebootcounters} -eq 1; then c4sys init; fi; \
run setkernaddr; if nboot.e ${bootload} 0 ${kernaddr}; then run setbootargs; \
setenv bootargs ${bootargs} video=${videomode} eth=${ethaddr}; \
if itest ${usewatchdogtimer} -eq 1; then c4sys watchdog c5986200 3; fi; \
bootm ${bootload}; else reset; fi; ";

/// Variables a factory unit does not have: the install's, and two left by
/// openHC's netboot bring-up. `tst`, `tstserverip` and `tstimage` ARE in
/// U-Boot's defaults (Control4's own), so a restore keeps them.
pub const NOT_FACTORY: &[&str] =
    &["ohcboot", "ohc_try", "ohc_addr", "ohc_bootargs", "oldbootcmd", "testboot"];

/// The environment an install writes, in order. `bootcmd` is last, so a failure
/// partway leaves U-Boot booting exactly what it booted before.
pub fn env_settings() -> Vec<(&'static str, String)> {
    vec![
        ("ohc_addr", format!("{SLOT_OFF:#x}")),
        ("ohc_bootargs", BOOTARGS.to_string()),
        ("ohcboot", OHCBOOT.to_string()),
        ("ohc_try", "0".to_string()),
        ("bootcmd", OPENHC_BOOTCMD.to_string()),
    ]
}

/// Which mtd node holds the slot, and at what offset, from `/proc/mtd`.
///
/// Stock's `mtd0` is the whole chip, so the slot is at [`SLOT_OFF`] in it.
/// openHC names the top half `openhc`, so the slot is at 0 in that partition.
pub fn slot(proc_mtd: &str, running: Running) -> Result<(String, u64), String> {
    let parts: Vec<(String, u64, String)> = proc_mtd
        .lines()
        .filter_map(|l| {
            let (dev, rest) = l.split_once(':')?;
            let mut f = rest.split_whitespace();
            let size = u64::from_str_radix(f.next()?, 16).ok()?;
            let _erase = f.next()?;
            let name = f.collect::<Vec<_>>().join(" ").trim_matches('"').to_string();
            Some((dev.to_string(), size, name))
        })
        .collect();
    match running {
        Running::Openhc => parts
            .iter()
            .find(|(_, _, n)| n == "openhc")
            .map(|(d, _, _)| (format!("/dev/{d}"), 0))
            .ok_or_else(|| "this openHC has no \"openhc\" partition; netboot a newer image first".into()),
        Running::Stock => match parts.iter().find(|(d, _, _)| d == "mtd0") {
            Some((_, size, _)) if *size == 0x2000_0000 => Ok(("/dev/mtd0".into(), SLOT_OFF)),
            _ => Err("stock mtd0 is not the whole 512 MiB chip; refusing to guess".into()),
        },
        _ => Err("needs stock Control4 or openHC running".into()),
    }
}

/// Check a uImage before it goes anywhere near the NAND: the magic, that it is
/// an IO Extender image, and that it fits the slot.
pub fn check_image(img: &[u8]) -> Result<(), String> {
    if img.len() < 64 || img[..4] != [0x27, 0x05, 0x19, 0x56] {
        return Err("not a U-Boot uImage".into());
    }
    let name = String::from_utf8_lossy(&img[32..64]);
    if !name.contains(IMAGE_NAME) {
        return Err(format!("uImage is \"{}\", not an IO Extender image", name.trim_end_matches('\0')));
    }
    if img.len() as u64 > SLOT_LEN {
        return Err(format!("image is {} bytes; the slot holds {SLOT_LEN}", img.len()));
    }
    Ok(())
}

/// The image padded with 0xFF to whole erase blocks: exactly what the slot reads
/// back as once it is written, so the box can compare the two by hash.
pub fn padded(img: &[u8]) -> Vec<u8> {
    let blocks = (img.len() as u64).div_ceil(BLOCK);
    let mut v = img.to_vec();
    v.resize((blocks * BLOCK) as usize, 0xff);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = "dev:    size   erasesize  name\n\
        mtd0: 20000000 00020000 \"NAND Flash Image (256M)\"\n\
        mtd6: 00020000 00020000 \"U-Boot Environment (128K)\"\n";
    const OPENHC: &str = "dev:    size   erasesize  name\n\
        mtd0: 00020000 00020000 \"u-boot-env\"\n\
        mtd13: 0ff80000 00020000 \"openhc\"\n";

    #[test]
    fn the_slot_is_found_on_both_systems() {
        assert_eq!(slot(STOCK, Running::Stock), Ok(("/dev/mtd0".into(), SLOT_OFF)));
        assert_eq!(slot(OPENHC, Running::Openhc), Ok(("/dev/mtd13".into(), 0)));
    }

    #[test]
    fn it_refuses_to_guess() {
        assert!(slot(OPENHC, Running::Stock).is_err(), "stock mtd0 must be the whole chip");
        assert!(slot(STOCK, Running::Openhc).is_err(), "an openHC with no openhc partition");
        assert!(slot(STOCK, Running::Unknown).is_err());
    }

    #[test]
    fn the_factory_bootcmd_is_stock_and_self_contained() {
        assert!(FACTORY_BOOTCMD.contains("c4sys init"));
        for v in NOT_FACTORY {
            assert!(!FACTORY_BOOTCMD.contains(&format!("{{{v}}}")), "{v} is unset by a restore");
            assert!(!FACTORY_BOOTCMD.contains(&format!("run {v}")), "{v} is unset by a restore");
        }
    }

    #[test]
    fn bootcmd_is_written_last() {
        assert_eq!(env_settings().last().map(|(k, _)| *k), Some("bootcmd"));
    }

    #[test]
    fn the_boot_script_counts_without_nesting_and_ends_on_stock() {
        assert!(!OHCBOOT.contains("else"), "U-Boot 1.2's shell: no nested if/else");
        assert!(OHCBOOT.trim_end().ends_with("run oldbootcmd"));
        assert!(OHCBOOT.contains(&format!("-ge {MAX_TRIES}")));
    }

    #[test]
    fn images_are_checked_and_padded() {
        let mut img = vec![0u8; 100];
        img[..4].copy_from_slice(&[0x27, 0x05, 0x19, 0x56]);
        assert!(check_image(&img).is_err(), "unnamed image");
        img[32..58].copy_from_slice(b"openHC ioxv1 (DM355 7.1.8)");
        assert_eq!(check_image(&img), Ok(()));
        let p = padded(&img);
        assert_eq!(p.len() as u64, BLOCK);
        assert!(p[100..].iter().all(|&b| b == 0xff));
        assert!(check_image(b"not a uimage at all, definitely not one, no").is_err());
    }
}
