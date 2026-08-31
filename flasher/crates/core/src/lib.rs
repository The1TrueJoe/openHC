//! openHC flasher — core domain knowledge.
//!
//! Pure, I/O-free, and heavily tested: the board matrix, the CEFDK/eMMC layout,
//! the autoscript format, image validation and the method-choice logic. The
//! rest of the app trusts this crate to be correct, so it carries no
//! dependencies beyond `serde` and every non-trivial fact has a test.

pub mod board;
pub mod cefdk;
pub mod image;
pub mod method;

pub use board::{Board, Family, Identity, Running};
pub use method::{Method, Plan};

#[cfg(test)]
mod tests {
    use super::*;

    // ---- autoscript framing: a bug here writes junk into flash --------------
    #[test]
    fn autoscript_is_nul_framed_and_padded() {
        let lines = cefdk::autoscript_for(0x980, 6_800_000, "console=ttyS0,115200 rw");
        assert_eq!(lines[0], "emmc rd 0x980 0x6000000 0x67c400");
        // the cache flush must survive: emmc rd DMAs without invalidating cache
        assert_eq!(lines[1], "cache flush");
        assert_eq!(lines[4], "bootlinux \"console=ttyS0,115200 rw\"");

        let blob = cefdk::build_autoscript(&lines).unwrap();
        assert_eq!(blob.len(), cefdk::MFH_SCRIPT_LEN);
        assert!(blob.starts_with(b"emmc rd 0x980 0x6000000 0x67c400\0"));
        // round-trips
        assert_eq!(cefdk::parse_autoscript(&blob), lines);
    }

    #[test]
    fn autoscript_refuses_to_overflow_the_mfh_entry() {
        let huge = vec!["x".repeat(3000)];
        assert!(cefdk::build_autoscript(&huge).is_err());
    }

    #[test]
    fn layout_is_self_consistent() {
        assert_eq!(cefdk::EMMC_KERNEL_OFF, cefdk::EMMC_CONTAINER_OFF + cefdk::EMMC_HEADER_LEN as u64);
        assert!(cefdk::EMMC_KERNEL_OFF < cefdk::P1_START);
        assert_eq!(cefdk::round_to_sector(1), 512);
        assert_eq!(cefdk::round_to_sector(512), 512);
        assert_eq!(cefdk::round_to_sector(513), 1024);
    }

    // ---- identification: a wrong id flashes the wrong image -----------------
    #[test]
    fn stock_ids_resolve_to_the_right_board() {
        assert_eq!(board::from_c4board(None, Some(2), Some(9)).board.unwrap().name, "ea3-v2");
        assert_eq!(board::from_c4board(None, Some(1), Some(5)).board.unwrap().name, "ea1-v1");
        assert_eq!(board::from_c4board(None, Some(0), Some(4)).board.unwrap().name, "ca1");
    }

    #[test]
    fn unknown_revision_stays_ambiguous_rather_than_guessing() {
        let id = board::from_c4board(None, Some(2), Some(99));
        assert!(!id.certain());
        assert!(id.candidates.len() > 1); // ea3-v1 and ea3-v2 both type 2
    }

    #[test]
    fn openhc_and_cefdk_identify_too() {
        let id = board::from_board_env(|k| (k == "OHC_BOARD").then(|| "ea3-v2".into()));
        assert_eq!(id.board.unwrap().name, "ea3-v2");
        assert_eq!(id.running, Running::Openhc);

        let id = board::from_cefdk_banner("Board : Type 1, Rev 5   MAC : 00:0f:ff");
        assert_eq!(id.board.unwrap().name, "ea1-v1");
        assert_eq!(id.running, Running::Cefdk);
    }

    #[test]
    fn secure_boot_drives_the_autoscript_choice() {
        assert!(board::by_name("ea3-v2").unwrap().needs_autoscript());
        assert!(!board::by_name("ea1-v1").unwrap().needs_autoscript());
    }

    // ---- method gating: the check that stops writing eMMC to an i.MX6 -------
    #[test]
    fn methods_refuse_boards_from_another_family() {
        let ea = board::from_c4board(None, Some(2), Some(9));
        let ca = board::from_c4board(None, Some(0), Some(4));

        assert!(Method::Network.suitable(&ea).is_ok());
        assert!(Method::Network.suitable(&ca).is_err());
        assert!(Method::Uboot.suitable(&ca).is_ok());
        assert!(Method::Uboot.suitable(&ea).is_err());

        // the chooser lands on the right method for each
        assert_eq!(method::choose(&ea, None).0, Some(Method::Network));
        assert_eq!(method::choose(&ca, None).0, Some(Method::Uboot));
    }

    // ---- container / size guard --------------------------------------------
    #[test]
    fn container_generates_a_blob_free_header() {
        let kernel = vec![0u8; 0x400];
        let blob = image::container(&kernel, None).unwrap();
        assert_eq!(blob.len(), cefdk::EMMC_HEADER_LEN + kernel.len());
        assert_eq!(&blob[0x10..0x14], &0x8086u32.to_le_bytes());
        // a generated header is almost entirely zero — no vendor signature bytes
        let header_sum: u32 = blob[..cefdk::EMMC_HEADER_LEN].iter().map(|&b| b as u32).sum();
        assert!(header_sum < 0x10000);
    }

    #[test]
    fn size_guard_matches_the_bootlinux_window() {
        assert_eq!(image::BOOTLINUX_WINDOW, 0x813000 - 0x100000);
        let probs = image::ea_problems(None, 0, false, false);
        assert!(probs.iter().any(|p| p.contains("no bzImage")));
        // an over-window kernel is rejected
        let head = {
            let mut h = vec![0u8; 0x400];
            h[0x1fe] = 0x55;
            h[0x1ff] = 0xaa;
            h[0x202..0x206].copy_from_slice(b"HdrS");
            h
        };
        let probs = image::ea_problems(Some(&head), image::BOOTLINUX_WINDOW + 1, true, true);
        assert!(probs.iter().any(|p| p.contains("bootlinux window")));
    }
}
