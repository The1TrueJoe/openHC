//! HC-800 install constants: the partitions, the GRUB entries, and the one
//! region that must never be written.
//!
//! This board is not like the others in this tool. It is a PC — AMI BIOS, GRUB
//! 0.97, a plain-text `menu.lst` — and **nothing in its boot chain verifies
//! anything**: no UEFI, no TPM, no module signing, no dm-verity. There is no
//! container to wrap, no fuse to work around and no autoscript to store. An
//! install is two file copies and one edited text file.
//!
//! That makes the interesting question safety, not access. See
//! <https://the1truejoe.github.io/openHC/shared/recovery/>.

/// GRUB's own stage1/stage2/menu.lst partition. **The single unrecoverable
/// failure on this board is corrupting it** — every recovery layer, including
/// the hardware factory-default button, needs GRUB to read `menu.lst` from
/// here. Back it up before writing, and read back what you wrote.
pub const GRUB_PART: &str = "/dev/sda1";
pub const GRUB_LABEL: &str = "bootload_fs_hc80";

/// The factory-restore system: its own kernel, its own rootfs, and the recorded
/// tarballs to rebuild both. **NEVER WRITTEN BY THIS TOOL, under any method.**
/// It is what makes everything else on this board reversible.
pub const RESTORE_PART: &str = "/dev/sda2";

/// The kernel-only ext3 partition, ~165 MB free. Control4 put the kernel here
/// because GRUB 0.97 cannot read ext4 extents and the vendor root is ext4 —
/// which is also why our kernel has somewhere roomy and GRUB-readable to live.
pub const KERNEL_PART: &str = "/dev/sda3";
pub const KERNEL_LABEL: &str = "kernel_fs_hc800";

/// The stock Control4 root. Untouched: openHC runs from an initramfs.
pub const VENDOR_ROOT_PART: &str = "/dev/sda4";

/// GRUB's name for [`KERNEL_PART`]. GRUB counts partitions from zero, so sda3
/// is `(hd0,2)` — the same value the vendor's own entry uses.
pub const GRUB_KERNEL_ROOT: &str = "(hd0,2)";

/// Where our files land on [`KERNEL_PART`], as GRUB will name them.
pub const KERNEL_FILE: &str = "/boot/openhc-bzImage";
pub const INITRD_FILE: &str = "/boot/openhc-initrd.gz";

/// Menu entry indices in the stock `menu.lst`, which we append to and never
/// reorder — entry 0 is what the hardware factory-default button selects, and
/// renumbering it would defeat the button.
pub const ENTRY_FACTORY: u8 = 0;
pub const ENTRY_VENDOR: u8 = 1;
pub const ENTRY_OPENHC: u8 = 2;

/// The two lines Control4 patched into their GRUB and the button depends on.
/// Touching either is how you break factory recovery, so the installer refuses
/// to write a `menu.lst` whose copies of these differ from what it read.
pub const GUARDED_LINES: &[&str] = &["support_factorydefault", "factorydefault"];

/// Kernel command line for an installed openHC.
///
/// `panic=10` is deliberately NOT here: it is compiled into the image
/// (`CONFIG_CMDLINE`), so it holds however the kernel was started, including
/// from a `kexec` typed by hand. Repeating it would only invite the two copies
/// to disagree.
pub const CMDLINE: &str = "console=ttyS0,115200";

/// The `menu.lst` entry for openHC.
///
/// `boot_once` decides the single most consequential thing about this install:
/// whether openHC is what the box RUNS, or what the box can be asked to run.
///
/// **`false` (the default).** openHC is GRUB's default entry and every boot is
/// openHC, so it survives a power cut with nothing to re-run. `fallback 1`
/// still covers a kernel that will not load at all. What it does NOT cover is a
/// kernel that loads and then panics: `panic=10` reboots into the same panic,
/// which is a loop that needs the ID button to break. That is the trade, and it
/// is why the network watchdog is off on this board — a dead uplink must not be
/// able to start one.
///
/// **`true`.** The entry ends `savedefault 1`, which GRUB executes BEFORE
/// handing over to the kernel, so every openHC boot immediately re-points the
/// default back at Control4. Any reset at all then lands on a system that
/// answers SSH, and a broken image costs one reboot instead of an unattended
/// loop. The right mode for a box you cannot reach, and for a kernel you do not
/// yet trust.
///
/// `savedefault` comes before the explicit `boot` in that mode, which is the
/// order the two vendor entries in this file already use — and the only order
/// that works, since `boot` does not return.
pub fn menu_entry(boot_once: bool) -> String {
    let save = if boot_once {
        format!("savedefault\t{ENTRY_VENDOR}\n")
    } else {
        String::new()
    };
    format!(
        "\ntitle\t\topenHC\nroot\t\t{GRUB_KERNEL_ROOT}\nkernel\t\t{KERNEL_FILE} {CMDLINE}\n\
         initrd\t\t{INITRD_FILE}\n{save}boot\n"
    )
}

/// GRUB Legacy's `default saved` reads this file, and `savedefault` rewrites
/// its first line in place.
///
/// In place is the operative phrase: GRUB records the file's BLOCK LIST at the
/// time `menu.lst` is parsed and writes those sectors directly, with no
/// filesystem driver behind it. So the file has to already exist at full size —
/// which is what the padding comment below is for, and why it must be created
/// once and then left alone rather than rewritten on every install.
pub fn default_file(entry: u8) -> String {
    format!(
        "{entry}\n#\n# This file is used by the grub-set-default command.\n\
         # openHC writes it so `default saved` has something to read.\n\
         # Do not move or edit it: savedefault rewrites these bytes directly,\n\
         # by sector, without going through the filesystem.\n"
    )
}
