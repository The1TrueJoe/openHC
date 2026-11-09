//! Install methods as data: which one applies to a board, and what it will do.
//!
//! The engine crate owns the *execution* of each method (it needs I/O); core
//! owns the *decision* — which method suits a board, and the human-readable
//! plan — so the choice logic is testable without a board attached.

use crate::board::{Board, Family, Identity};

/// A method's identity and the family it understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Over SSH into a running system: write the eMMC container and, on a
    /// secure-boot part, the MFH autoscript. No serial, no button.
    Network,
    /// Copy zImage + DTB + boot.scr onto the CA-1's vfat partition.
    Uboot,
    /// Drive the CEFDK shell over serial. Needs a console and the ID button;
    /// the fallback when nothing is running to ssh into.
    Serial,
    /// HC-800: start openHC out of the running system with `kexec`. Writes to
    /// NO partition at all — not the bootloader, not the vendor root, not the
    /// factory-restore image — so the box is byte-identical afterwards and a
    /// power cycle returns it to stock. The safest install in this tool, and
    /// the one to reach for first on that board.
    Kexec,
    /// HC-800: the persistent install. Copies the kernel and initramfs onto the
    /// spare ext3 kernel partition and appends a `menu.lst` entry, so openHC
    /// survives a power cut. This is the only method here that writes the
    /// bootloader partition.
    Grub,
    /// IO Extender: write openHC to the half of the NAND stock never
    /// partitioned and make U-Boot boot it, falling back to stock after three
    /// failed boots. Works from stock or openHC, over SSH.
    Nand,
    /// HC-250: openHC on the interior SD card, booted by stock U-Boot
    /// (`bootmode=ohc`), falling back to stock after three failed boots. The SD
    /// and EEPROM are written from openHC in RAM, the U-Boot env by stock.
    Sd,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Network => "network",
            Method::Uboot => "uboot",
            Method::Serial => "serial",
            Method::Kexec => "kexec",
            Method::Grub => "grub",
            Method::Nand => "nand",
            Method::Sd => "sd",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Method::Network => "over SSH — no serial, no button (default)",
            Method::Uboot => "copy files to the vfat partition — no serial (CA-1)",
            Method::Serial => "via the CEFDK shell — needs a serial console and the ID button",
            Method::Kexec => "run it from RAM — writes nothing to disk, reverts on a power cycle",
            Method::Grub => "install to the kernel partition — survives a power cut (HC-800)",
            Method::Nand => "install to the unused half of the NAND — falls back to stock on its own (IOX)",
            Method::Sd => "install to the interior SD card — falls back to stock on its own (HC-250)",
        }
    }

    fn families(self) -> &'static [Family] {
        match self {
            Method::Network | Method::Serial => &[Family::Ea],
            Method::Uboot => &[Family::Ca],
            Method::Kexec => &[Family::Hc, Family::Omap],
            Method::Grub => &[Family::Hc],
            Method::Nand => &[Family::Iox],
            Method::Sd => &[Family::Omap],
        }
    }

    /// True when the method leaves every partition byte-identical. Only
    /// [`Method::Kexec`] qualifies, and the UI leans on it to say so out loud
    /// before anyone commits to something that does write.
    pub fn writes_nothing(self) -> bool {
        matches!(self, Method::Kexec)
    }

    /// Can this method install onto this identity? Returns the reason when not,
    /// so the UI can explain why an option is greyed out.
    pub fn suitable(self, id: &Identity) -> Result<(), String> {
        use crate::board::Running::*;
        let board = match id.board {
            Some(b) => b,
            None => {
                // The serial method can still work from the CEFDK shell, where
                // there is no OS to identify a board from yet.
                if self == Method::Serial && id.running == Cefdk {
                    return Ok(());
                }
                return Err("board not identified; refusing to guess an image".into());
            }
        };
        if !self.families().contains(&board.family) {
            return Err(format!(
                "{} is {:?}-family; this method does not apply",
                board.name, board.family
            ));
        }
        if matches!(self, Method::Network | Method::Uboot | Method::Kexec | Method::Grub | Method::Nand | Method::Sd)
            && !matches!(id.running, Stock | Openhc)
        {
            return Err("needs a running system to log into".into());
        }
        Ok(())
    }
}

/// Methods in preference order — cheapest-for-the-user first, so "the default
/// needs no cable and no button" falls out of the ordering rather than a
/// special case.
///
/// Every board's default is a PERSISTENT install — what the box runs after a
/// power cut — so on the HC-800 Grub sits ahead of Kexec. Kexec (writes
/// nothing, gone on a power cycle) is there to pick with `--method kexec` for
/// trying an image. Grub was proven safe to land on by default: `fallback 1`
/// catches a kernel that will not load, and both the ID button and
/// `ohc-flash restore` run Control4's factory restore once, which wipes our
/// files from sda3 and leaves the next boot falling back to stock.
pub const METHODS: &[Method] =
    &[Method::Network, Method::Uboot, Method::Grub, Method::Sd, Method::Kexec, Method::Nand, Method::Serial];

/// Choose a method for an identity, optionally forced. Returns the chosen
/// method and the reasons the others were rejected (for display).
pub fn choose(id: &Identity, prefer: Option<Method>) -> (Option<Method>, Vec<(Method, String)>) {
    let mut rejected = vec![];
    for &m in METHODS {
        if let Some(p) = prefer {
            if m != p {
                continue;
            }
        }
        match m.suitable(id) {
            Ok(()) => return (Some(m), rejected),
            Err(why) => rejected.push((m, why)),
        }
    }
    (None, rejected)
}

/// A step in a plan, shown to the user before anything is written.
#[derive(Debug, Clone)]
pub struct Plan {
    pub method: Method,
    pub steps: Vec<String>,
    /// What gets written, phrased as a warning.
    pub writes: Vec<String>,
    pub needs_serial: bool,
    pub needs_button: bool,
    pub reversible: String,
}

/// Build the plan text for a board + method. Pure: the same summary the UI and
/// the CLI both render.
pub fn plan(board: &Board, method: Method) -> Plan {
    use crate::cefdk::*;
    match method {
        Method::Network => {
            let mut steps = vec![
                format!(
                    "write kernel container -> eMMC {EMMC_CONTAINER_OFF:#x} (+ length at {EMMC_SIZE_OFF:#x})"
                ),
                "write the tiny networked boot-init -> eMMC 0x800000".into(),
            ];
            let mut writes = vec![format!(
                "eMMC {EMMC_SIZE_OFF:#x}, {EMMC_CONTAINER_OFF:#x}, 0x800000-0x1000000 and one \
                 sector at 0x1000000 (raw, ahead of p1)"
            )];
            if board.needs_autoscript() {
                steps.push(format!(
                    "write CEFDK autoscript -> /dev/mtd0 {MFH_SCRIPT_OFF:#x} \
                     (bootlinux, bypasses the blown secure-boot fuse)"
                ));
                writes.push(format!("/dev/mtd0 {MFH_SCRIPT_OFF:#x} ({MFH_SCRIPT_LEN} bytes)"));
            } else {
                steps.push("no autoscript needed — fuse is clear, bootkernel takes the container".into());
            }
            steps.push("reboot into boot-init, then stream rootfs.ext2 to p1 over SSH".into());
            Plan {
                method,
                steps,
                writes,
                needs_serial: false,
                needs_button: false,
                reversible: "the recessed factory-restore button reimages kernel + rootfs from \
                             p2; p2 is never written by this tool"
                    .into(),
            }
        }
        Method::Uboot => Plan {
            method,
            steps: vec![
                "mount the vfat 'kernel' partition (mmcblk1p1)".into(),
                "copy zImage, DTB and boot.scr onto it".into(),
                "reboot — U-Boot's stock bootcmd picks up boot.scr".into(),
            ],
            writes: vec!["mmcblk1p1: zImage, *.dtb, boot.scr (files, not raw offsets)".into()],
            needs_serial: false,
            needs_button: false,
            reversible: "delete boot.scr from the vfat partition; the stock boot path returns"
                .into(),
        },
        Method::Kexec if board.family == Family::Omap => Plan {
            method,
            steps: vec![
                "copy ohc_kexec.ko + ohc-kexec.sh to stock's /tmp and insmod the module".into(),
                "stream the uImage into /dev/ohc-kexec (page chunks in RAM; /tmp is too small)".into(),
                "stop WDT2, shut stock down, and jump into openHC with stock's ATAGs".into(),
            ],
            writes: vec!["nothing. The image lives in RAM pages the module holds".into()],
            needs_serial: false,
            needs_button: false,
            reversible: "a reboot or power cycle boots stock again: U-Boot, its env and the NAND \
                         are never touched"
                .into(),
        },
        Method::Sd => {
            use crate::hc250::*;
            Plan {
                method,
                steps: vec![
                    format!(
                        "on stock: write U-Boot's env with stock's own fw_setenv (bootohc, the restoresystem \
                         hook, bootmode=ohc last) and save Control4's IO firmware tree; U-Boot keeps booting \
                         stock until openHC arms EEPROM byte {TRY_BYTE}"
                    ),
                    "kexec openHC into RAM".into(),
                    format!(
                        "on openHC: shrink SD p2 to 2 GiB, add p3 vfat {BOOT_LABEL} and p4 ext4 {DATA_LABEL} \
                         (once), write {SD_IMAGE} to p3, arm EEPROM byte {TRY_BYTE}"
                    ),
                    "reboot into openHC from the SD, and keep the IO tree on p4".into(),
                    "(from openHC in RAM, a reboot to stock does the env half first; from openHC on the SD, \
                     only the image is replaced)"
                        .into(),
                ],
                writes: vec![
                    "the interior SD card: p2 resized (its files kept), p3 and p4 created; p1 untouched".into(),
                    format!("EEPROM bytes {TRY_BYTE} and {REQUEST_BYTE} (openHC's), and stock's boot counters 0-2"),
                    "U-Boot env (written by stock's fw_setenv, never from Linux)".into(),
                ],
                needs_serial: false,
                needs_button: false,
                reversible: format!(
                    "{MAX_TRIES} failed openHC boots in a row boot stock on their own. The factory-restore \
                     button, or `ohc-flash restore`, puts Control4's IO firmware and the recovery image back \
                     and sets bootmode=nand; `ohc-flash uninstall` only sets bootmode=nand"
                ),
            }
        }
        Method::Kexec => Plan {
            method,
            steps: vec![
                "mount a tmpfs sized to the release at /mnt/ohc-stage and copy bzImage + rootfs.cpio.gz (+ a static kexec if the box has none) into it".into(),
                "kexec -l (stage into the running kernel — still nothing written)".into(),
                "kexec -e (jump straight into openHC, skipping BIOS and GRUB)".into(),
            ],
            writes: vec!["nothing. The stage is a tmpfs (RAM); no partition is opened for writing".into()],
            needs_serial: false,
            needs_button: false,
            reversible: "a power cycle. GRUB, the vendor root and the factory-restore image are \
                         never touched, so there is nothing to revert"
                .into(),
        },
        Method::Grub => {
            use crate::hc800::*;
            Plan {
                method,
                steps: vec![
                    format!("mount {KERNEL_PART} ({KERNEL_LABEL}) and copy the kernel + initramfs to /boot"),
                    format!("mount {GRUB_PART} ({GRUB_LABEL}) and append entry {ENTRY_OPENHC} to menu.lst"),
                    format!(
                        "set `default {ENTRY_OPENHC}`, so every boot runs openHC (with --boot-once: \
                         `default saved`, and the entry's `savedefault {ENTRY_VENDOR}` hands each boot \
                         back to Control4)"
                    ),
                    "NOT a reboot: the box keeps running whatever it is running until you restart \
                     it (`ohc-flash boot` does that)"
                        .into(),
                ],
                writes: vec![
                    format!("{KERNEL_PART}: {KERNEL_FILE}, {INITRD_FILE} (files, on the spare kernel partition)"),
                    format!("{GRUB_PART}: /boot/grub/menu.lst (and /boot/grub/default with --boot-once)"),
                ],
                needs_serial: false,
                needs_button: false,
                reversible: format!(
                    "`ohc-flash uninstall` puts back the as-shipped menu.lst; `ohc-flash restore` runs \
                     Control4's factory restore once. `fallback {ENTRY_VENDOR}` catches a kernel that will \
                     not load, and holding the ID button at power-on boots entry {ENTRY_FACTORY} (the \
                     factory restore) regardless of menu.lst. {RESTORE_PART} is never written"
                ),
            }
        }
        Method::Nand => {
            use crate::iox::*;
            Plan {
                method,
                steps: vec![
                    "upload the image (padded to whole erase blocks) to a RAM staging area".into(),
                    format!("erase {SLOT_BLOCKS} blocks at NAND {SLOT_OFF:#x} and write it, refusing a slot with a bad block"),
                    "read the slot back through ECC and compare its hash with the staged image".into(),
                    format!(
                        "only then set U-Boot's environment: ohcboot, ohc_try=0, and bootcmd=`{OPENHC_BOOTCMD}` last"
                    ),
                    "NOT a reboot: the box keeps running until you restart it".into(),
                ],
                writes: vec![
                    format!("NAND {SLOT_OFF:#x}..+{SLOT_LEN:#x}, in the half stock never partitioned"),
                    "U-Boot environment (both redundant copies)".into(),
                ],
                needs_serial: false,
                needs_button: false,
                reversible: format!(
                    "no stock partition, bootloader or recovery image is touched. {MAX_TRIES} openHC boots in a row that never bring the uplink up (panic, hang, no network) make U-Boot boot stock on its own; `ohc-flash uninstall` sets bootcmd back to `{STOCK_BOOTCMD}`"
                ),
            }
        }
        Method::Serial => Plan {
            method,
            steps: vec![
                "wait for the manufacturing-mode shell (hold ID button, power-cycle)".into(),
                format!("tftp bzImage -> RAM {KERNEL_ADDR:#x}"),
                format!("emmc wr -> raw gap {GAP_OFF_DEFAULT:#x}"),
                "store CEFDK autoscript".into(),
                "reset".into(),
            ],
            writes: vec![
                format!("eMMC raw gap {GAP_OFF_DEFAULT:#x}"),
                "SPI-NOR MFH script entry".into(),
            ],
            needs_serial: true,
            needs_button: true,
            reversible: "`script off` at the CEFDK shell, or the factory-restore button".into(),
        },
    }
}
