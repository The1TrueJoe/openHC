//! openHC flasher — core domain knowledge.
//!
//! Pure, I/O-free, and heavily tested: the board matrix, the CEFDK/eMMC layout,
//! the autoscript format, image validation and the method-choice logic. The
//! rest of the app trusts this crate to be correct, so it carries no
//! dependencies beyond `serde` and every non-trivial fact has a test.

pub mod board;
pub mod cefdk;
pub mod hc800;
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

    // ---- HC-800: which method, and what it promises -------------------------
    //
    // These are the decisions that decide whether a bootloader partition gets
    // written, so they are worth pinning even though they look obvious. The
    // ordering one especially: `kexec` sitting ahead of `grub` is the reason a
    // user who never picks a method lands on the option that writes nothing.

    fn ident(name: &str, running: Running) -> Identity {
        let b = board::by_name(name).expect("board in the table");
        Identity { board: Some(b), candidates: vec![b], running, version: None, raw: vec![] }
    }

    #[test]
    fn hc800_defaults_to_the_method_that_writes_nothing() {
        let (m, _) = method::choose(&ident("hc800", Running::Stock), None);
        assert_eq!(m, Some(Method::Kexec));
        assert!(Method::Kexec.writes_nothing());
        assert!(!Method::Grub.writes_nothing());
    }

    #[test]
    fn the_hc800_methods_do_not_leak_onto_other_families() {
        for name in ["ea1-v1", "ea3-v2", "ca1", "ioxv1"] {
            let id = ident(name, Running::Stock);
            assert!(Method::Kexec.suitable(&id).is_err(), "kexec offered on {name}");
            assert!(Method::Grub.suitable(&id).is_err(), "grub offered on {name}");
        }
        // ... and the EA/CA methods stay off the HC-800.
        let hc = ident("hc800", Running::Stock);
        assert!(Method::Network.suitable(&hc).is_err());
        assert!(Method::Uboot.suitable(&hc).is_err());
    }

    #[test]
    fn both_hc800_methods_need_something_to_log_into() {
        // There is no serial fallback on this board: the CEFDK shell does not
        // exist here, so an unreachable HC-800 is a power-cycle, not a method.
        let cefdk = ident("hc800", Running::Cefdk);
        assert!(Method::Kexec.suitable(&cefdk).is_err());
        assert!(Method::Grub.suitable(&cefdk).is_err());
        for r in [Running::Stock, Running::Openhc] {
            assert!(Method::Kexec.suitable(&ident("hc800", r)).is_ok());
            assert!(Method::Grub.suitable(&ident("hc800", r)).is_ok());
        }
    }

    #[test]
    fn the_kexec_plan_promises_no_writes_and_the_grub_plan_names_every_one() {
        let b = board::by_name("hc800").unwrap();

        let k = method::plan(b, Method::Kexec);
        assert_eq!(k.writes.len(), 1);
        assert!(k.writes[0].starts_with("nothing"), "{:?}", k.writes);
        assert!(!k.needs_serial && !k.needs_button);

        let g = method::plan(b, Method::Grub);
        // Exactly the two partitions, and NEVER the factory-restore one.
        assert!(g.writes.iter().any(|w| w.contains(hc800::KERNEL_PART)));
        assert!(g.writes.iter().any(|w| w.contains(hc800::GRUB_PART)));
        assert!(
            !g.writes.iter().any(|w| w.contains(hc800::RESTORE_PART)),
            "the factory-restore partition must never appear in a write list"
        );
    }

    // ---- the menu.lst entry: this text is the safety property ---------------
    #[test]
    fn boot_once_hands_the_default_back_before_it_boots() {
        let e = hc800::menu_entry(true);
        let sd = e.find("savedefault").expect("entry must savedefault");
        let bt = e.find("\nboot").expect("entry must end with boot");
        // savedefault BEFORE boot, because boot does not return.
        assert!(sd < bt, "savedefault must precede boot:\n{e}");
        // ... and it must hand back to the VENDOR entry, not to itself. Pointing
        // it at ENTRY_OPENHC would make every reset a loop into the same fault.
        assert!(e.contains(&format!("savedefault\t{}", hc800::ENTRY_VENDOR)));
        assert!(!e.contains(&format!("savedefault\t{}", hc800::ENTRY_OPENHC)));
    }

    #[test]
    fn the_persistent_entry_does_not_save_anything() {
        // A stray savedefault here would silently turn the default install back
        // into a boot-once, and the symptom is "it went back to Control4" a
        // reboot later — which reads as the install having failed.
        let e = hc800::menu_entry(false);
        assert!(!e.contains("savedefault"), "persistent entry must not savedefault:\n{e}");
        assert!(e.trim_end().ends_with("boot"));
    }

    #[test]
    fn both_modes_name_the_kernel_and_the_initramfs() {
        for once in [true, false] {
            let e = hc800::menu_entry(once);
            assert!(e.contains(hc800::KERNEL_FILE), "mode boot_once={once}");
            assert!(e.contains(hc800::INITRD_FILE), "mode boot_once={once}");
            assert!(e.contains(hc800::GRUB_KERNEL_ROOT));
        }
    }

    #[test]
    fn the_default_file_leads_with_the_entry_number() {
        // GRUB rewrites only the first line, in place, by sector — so the number
        // has to be first and the rest is padding that must not move.
        let f = hc800::default_file(hc800::ENTRY_OPENHC);
        assert_eq!(f.lines().next().unwrap(), hc800::ENTRY_OPENHC.to_string());
        assert!(f.len() > 32, "needs padding for savedefault to rewrite in place");
    }

    // ---- identification -----------------------------------------------------
    #[test]
    fn dmi_identifies_the_hc800_and_nothing_else_does() {
        let id = board::from_dmi(Some("Lite-On Tech."), Some("HC800"), Running::Stock);
        assert_eq!(id.board.map(|b| b.name), Some("hc800"));
        assert!(id.certain());
        // Case and whitespace come out of a BIOS table nobody proofread.
        let sloppy = board::from_dmi(Some(" lite-on tech. "), Some("hc800\n"), Running::Stock);
        assert_eq!(sloppy.board.map(|b| b.name), Some("hc800"));
        // A different PC must not match.
        let other = board::from_dmi(Some("Dell Inc."), Some("OptiPlex"), Running::Stock);
        assert!(other.board.is_none());
    }

    #[test]
    fn ohc_model_carries_a_board_name_outside_the_ea_family() {
        // The EA overlays write a bare 1/3/5; every other board writes its own
        // name. Reading only the former left a running openHC HC-800
        // unidentified, and the flasher then refused to touch it.
        let hc = board::from_board_env(|k| (k == "OHC_MODEL").then(|| "hc800".into()));
        assert_eq!(hc.board.map(|b| b.name), Some("hc800"));
        assert_eq!(hc.running, Running::Openhc);

        let ea = board::from_board_env(|k| (k == "OHC_MODEL").then(|| "3".into()));
        assert!(ea.candidates.iter().all(|b| b.family == Family::Ea));
        assert!(!ea.candidates.is_empty());
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
