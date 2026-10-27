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

/// The stock Control4 root. openHC runs from an initramfs and keeps only its
/// `/openhc` data directory here (S08ohcdata); `uninstall` also makes sure the
/// `/tmp` mount point stock's fstab needs is present (see [`VENDOR_TMP_MODE`]).
pub const VENDOR_ROOT_PART: &str = "/dev/sda4";

/// Stock mounts a tmpfs on `/tmp`, so the root needs the directory itself, and
/// without it Director crash-loops every ~20 s. Control4's own factory restore
/// never creates it: `root-fs-sherman.tgz` on sda2 has no `tmp` entry, and
/// neither `restore.sh` nor stock's `rcS` makes one (checked 2026-10-26).
pub const VENDOR_TMP_MODE: &str = "1777";

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

/// Title of the one-shot factory-restore entry [`factory_once_menu`] appends.
/// Deliberately NOT starting with "openHC": the install and boot paths look for
/// `title\t\topenHC` to find our kernel entry, and must never mistake this one
/// for it.
pub const FACTORY_ONCE_TITLE: &str = "HC-800 Factory Default Image (once)";

/// Title of the one-shot first stock boot [`factory_once_menu`] appends after
/// the restore entry. Not starting with "openHC", for the same reason.
pub const STOCK_FIRSTBOOT_TITLE: &str = "HC-800 Image (first boot after restore, once)";

/// Both one-shot entries a restore leaves in `menu.lst`. Spent once the box is
/// back on stock; the install drops them so openHC lands at [`ENTRY_OPENHC`].
pub const RESTORE_ONCE_TITLES: &[&str] = &[FACTORY_ONCE_TITLE, STOCK_FIRSTBOOT_TITLE];

/// What the first stock boot after a restore runs first, inside Control4's
/// kernel-embedded initramfs, before that initramfs's own `/init`.
///
/// Control4's restore never creates `/tmp`: `root-fs-sherman.tgz` on sda2 has
/// no `tmp` entry, and neither `restore.sh` nor stock's `rcS` makes one, so
/// `mount -a` cannot put fstab's tmpfs there and Director restarts every ~20 s.
///
/// **`rdinit=`, not `init=`.** Stock's kernel carries its own initramfs, whose
/// `/init` mounts sda4 and `switch_root`s to `/sbin/init`. With an initramfs the
/// kernel runs `/init` and leaves `init=` to it, and that `/init` parses `init=`
/// into a variable it never uses (it reads `$init`, switches to `$INIT`), so an
/// `init=` here was ignored on hardware (2026-10-26). `rdinit=` is the kernel's
/// own choice of that first program, so the shell below runs instead, makes
/// `/tmp` (1777: `mkdir -m`, since the restore never leaves one to chmod) on
/// sda4 with the initramfs's busybox, undoes its mounts, and execs
/// Control4's `/init`, which then boots exactly as it always does. Every step is
/// `;`-joined so a failure still reaches `exec /init`.
///
/// The kernel hands everything after `--` to that program as arguments, and its
/// command-line parser keeps a double-quoted argument whole, so `/bin/sh` gets
/// `-c` and the script. **Hence the trailing ` #`:** this 3.16 kernel's
/// `next_arg` drops a bare quoted argument's opening `"` but keeps its closing
/// one (it strips both only for a `name="value"` argument), so sh is handed
/// `…exec /init #"`, and the `#` makes that stray quote a comment. Without it
/// sh refused the script ("unterminated quoted string"), PID 1 exited and the
/// kernel panicked, on the unit and with stock's own bzImage under QEMU
/// (2026-10-26). A kernel that strips both quotes leaves an empty comment.
/// The script must hold no other double quote, and no `root=`, `type=` or
/// `debug=`: Control4's `/init` reads those words from `/proc/cmdline`, script
/// included. `mount` names `ext4` because nothing has mounted `/proc` for
/// busybox to probe filesystem types from.
///
/// **It is terse on purpose.** GRUB 0.97 copies at most 255 bytes of a kernel
/// line's arguments to Linux, so [`MAX_KERNEL_ARGS`] keeps a margin under that.
/// `panic=10` is why a failure costs one reboot rather than a hung box: stock's
/// kernel has no panic timeout of its own, and GRUB has already saved entry
/// [`ENTRY_VENDOR`], so the reboot is plain stock.
pub const STOCK_FIRSTBOOT_INIT: &str = "panic=10 rdinit=/bin/sh -- -c \"mount -t devtmpfs d /dev;\
     mkdir /r;mount -t ext4 /dev/sda4 /r;mkdir -m1777 /r/tmp;umount /r;umount /dev;exec /init #\"";

/// The longest argument string (everything after the kernel's path) a stock
/// entry may carry: under the 255 bytes GRUB 0.97 passes on, with room to spare.
pub const MAX_KERNEL_ARGS: usize = 240;

/// Rewrite a `menu.lst` so the next boot runs Control4's factory restore
/// EXACTLY ONCE, returning the new text and the index of the entry to save as
/// the default.
///
/// **Why not just `default 0`.** Entry 0 is what the ID button picks, but the
/// button overrides the default for one boot and leaves `menu.lst` alone. The
/// restore system (`/etc/restore.sh` on sda2) re-images sda3/sda4 and reboots
/// and never touches `menu.lst` — so a `default 0` written by software is a
/// restore that runs forever. Observed on the unit 2026-10-02: four full
/// restore cycles back to back, each one ending in `Default set to 0.`
///
/// **What this does instead.** It appends a copy of entry 0 — same root, same
/// kernel line, so the restore is the button's restore byte for byte — with
/// `savedefault` before its `boot`, and switches the file to `default saved`.
/// GRUB runs `savedefault` before handing over, so the restore kernel starts
/// with the default already pointing at the next entry, and the reboot
/// `restore.sh` ends with lands there. The same mechanism the openHC boot-once
/// entry already relies on.
///
/// **The next entry is stock's first boot.** A copy of entry 1, Control4's own
/// kernel on the freshly restored sda3, with [`STOCK_FIRSTBOOT_INIT`] on its
/// kernel line and `savedefault {ENTRY_VENDOR}`. It creates the `/tmp` the
/// restore never makes and then runs stock's normal boot, so the box comes back
/// healthy with no extra reboot and nothing logging in to it. Every later boot
/// is plain entry 1. If that boot ever failed, the default is already back on
/// entry 1, so the next power cycle is the plain stock boot a restore gave
/// before this entry existed.
///
/// Entries 0 and 1, `support_factorydefault` and `factorydefault` are left as
/// they were. Any openHC kernel entry is dropped — the restore reformats sda3,
/// so it would name files that no longer exist — as are earlier copies of both
/// one-shot entries, so running it twice does not stack them up.
pub fn factory_once_menu(menu: &str) -> Result<(String, u8), String> {
    for g in GUARDED_LINES {
        if !menu.lines().any(|l| l.trim_start().starts_with(g)) {
            return Err(format!("menu.lst has no `{g}` line — the factory-default button depends on it"));
        }
    }

    // Split into the header (everything before the first `title`) and entries,
    // keeping every line as it was.
    let mut header: Vec<&str> = Vec::new();
    let mut entries: Vec<Vec<&str>> = Vec::new();
    for l in menu.lines() {
        if l.trim_start().starts_with("title") {
            entries.push(vec![l]);
        } else if let Some(e) = entries.last_mut() {
            e.push(l);
        } else {
            header.push(l);
        }
    }
    let title_of = |e: &Vec<&str>| e[0].trim_start()["title".len()..].trim().to_string();
    entries.retain(|e| {
        let t = title_of(e);
        !t.starts_with("openHC") && !RESTORE_ONCE_TITLES.contains(&t.as_str())
    });
    if entries.len() <= ENTRY_VENDOR as usize {
        return Err(format!("menu.lst has {} vendor entries, expected at least {}", entries.len(), ENTRY_VENDOR + 1));
    }
    let factory = &entries[ENTRY_FACTORY as usize];
    if !factory.iter().any(|l| l.contains("restore_fs")) {
        return Err(format!("entry {ENTRY_FACTORY} does not boot restore_fs — not the menu.lst this tool expects"));
    }
    let vendor = &entries[ENTRY_VENDOR as usize];
    if !vendor.iter().any(|l| l.trim_start().starts_with("kernel") && l.contains("root=root_fs")) {
        return Err(format!("entry {ENTRY_VENDOR} does not boot root_fs — not the menu.lst this tool expects"));
    }
    if vendor.iter().any(|l| l.contains("init=")) {
        return Err(format!("entry {ENTRY_VENDOR} already sets init= or rdinit= — not the menu.lst this tool expects"));
    }

    let mut seen_default = false;
    let mut out = String::new();
    for l in &header {
        if l.trim_start().starts_with("default") {
            seen_default = true;
            out.push_str("default\t\tsaved\n");
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !seen_default {
        return Err("menu.lst has no `default` line at all — not the file this tool expects".into());
    }
    for e in &entries {
        for l in e {
            out.push_str(l);
            out.push('\n');
        }
    }

    // The copies: an entry's lines minus its title, trailing blanks and
    // `boot`, then savedefault and boot in the only order that works.
    let body = |e: &Vec<&str>| -> Vec<String> {
        let mut b: Vec<String> = e[1..].iter().map(|l| l.to_string()).collect();
        while b.last().is_some_and(|l| l.trim().is_empty() || l.trim() == "boot") {
            b.pop();
        }
        b
    };
    let once = entries.len() as u8;
    let firstboot = once + 1;
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&format!("title\t\t{FACTORY_ONCE_TITLE}\n"));
    for l in body(factory) {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str(&format!("savedefault\t{firstboot}\nboot\n\n"));
    out.push_str(&format!("title\t\t{STOCK_FIRSTBOOT_TITLE}\n"));
    for l in body(vendor) {
        if l.trim_start().starts_with("kernel") {
            out.push_str(&format!("{} {STOCK_FIRSTBOOT_INIT}\n", l.trim_end()));
        } else {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out.push_str(&format!("savedefault\t{ENTRY_VENDOR}\nboot\n"));
    Ok((out, once))
}

/// The `title` of every entry in `menu`, in GRUB's order (index = entry number).
pub fn titles(menu: &str) -> Vec<String> {
    menu.lines()
        .filter_map(|l| l.trim_start().strip_prefix("title"))
        .map(|t| t.trim().to_string())
        .collect()
}

/// `menu` with the entry titled exactly `title` removed (from its `title` line
/// up to the next one), every other line untouched.
///
/// The install uses this to drop a spent [`FACTORY_ONCE_TITLE`] entry: once
/// the restore it started has run, the saved default already points at the
/// vendor entry, and leaving it in place would push a newly appended openHC
/// entry to index 3 while everything that boots openHC names
/// [`ENTRY_OPENHC`] — so "boot openHC" would start a factory restore instead.
pub fn drop_entry(menu: &str, title: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for l in menu.lines() {
        if let Some(t) = l.trim_start().strip_prefix("title") {
            skipping = t.trim() == title;
        }
        if !skipping {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
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
    default_file_body(entry)
}

/// The entry number a `default` file names, as GRUB reads it: the leading
/// digits of the first line.
///
/// GRUB's `savedefault` writes the number and then a NUL (`1\0\n\n…` on the
/// unit), and busybox `sed`'s `.*` stops at that NUL, so a first line rewritten
/// on the box reads back as `2\0`. GRUB stops at the first non-digit, and this
/// does the same, so a read-back check agrees with what GRUB will boot.
pub fn default_entry(text: &str) -> Option<u8> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn default_file_body(entry: u8) -> String {
    format!(
        "{entry}\n#\n# This file is used by the grub-set-default command.\n\
         # openHC writes it so `default saved` has something to read.\n\
         # Do not move or edit it: savedefault rewrites these bytes directly,\n\
         # by sector, without going through the filesystem.\n"
    )
}
