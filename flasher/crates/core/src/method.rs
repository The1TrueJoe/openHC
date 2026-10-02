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
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Network => "network",
            Method::Uboot => "uboot",
            Method::Serial => "serial",
            Method::Kexec => "kexec",
            Method::Grub => "grub",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Method::Network => "over SSH — no serial, no button (default)",
            Method::Uboot => "copy files to the vfat partition — no serial (CA-1)",
            Method::Serial => "via the CEFDK shell — needs a serial console and the ID button",
            Method::Kexec => "run it from RAM — writes nothing to disk, reverts on a power cycle",
            Method::Grub => "install to the kernel partition — survives a power cut (HC-800)",
        }
    }

    fn families(self) -> &'static [Family] {
        match self {
            Method::Network | Method::Serial => &[Family::Ea],
            Method::Uboot => &[Family::Ca],
            Method::Kexec | Method::Grub => &[Family::Hc],
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
        if matches!(self, Method::Network | Method::Uboot | Method::Kexec | Method::Grub)
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
/// Kexec sits ahead of Grub for the same reason Network sits ahead of Serial:
/// on the board where both apply, the one that writes nothing is the one a user
/// should land on without having to choose it.
pub const METHODS: &[Method] =
    &[Method::Network, Method::Uboot, Method::Kexec, Method::Grub, Method::Serial];

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
            let mut steps = vec![format!(
                "write kernel container -> eMMC {EMMC_CONTAINER_OFF:#x} (+ length at {EMMC_SIZE_OFF:#x})"
            )];
            let mut writes =
                vec![format!("eMMC {EMMC_SIZE_OFF:#x} and {EMMC_CONTAINER_OFF:#x} (raw, ahead of p1)")];
            if board.needs_autoscript() {
                steps.push(format!(
                    "write CEFDK autoscript -> /dev/mtd0 {MFH_SCRIPT_OFF:#x} \
                     (bootlinux, bypasses the blown secure-boot fuse)"
                ));
                writes.push(format!("/dev/mtd0 {MFH_SCRIPT_OFF:#x} ({MFH_SCRIPT_LEN} bytes)"));
            } else {
                steps.push("no autoscript needed — fuse is clear, bootkernel takes the container".into());
            }
            steps.push("reboot to the RAM installer, then write rootfs to p1".into());
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
        Method::Kexec => Plan {
            method,
            steps: vec![
                "copy bzImage + rootfs.cpio.gz to /tmp on the running system".into(),
                "kexec -l (stage into the running kernel — still nothing written)".into(),
                "kexec -e (jump straight into openHC, skipping BIOS and GRUB)".into(),
            ],
            writes: vec!["nothing. /tmp is a tmpfs; no partition is opened for writing".into()],
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
                        "switch `default {ENTRY_VENDOR}` to `default saved`, so the openHC entry's \
                         `savedefault {ENTRY_VENDOR}` makes every openHC boot a BOOT-ONCE"
                    ),
                    format!(
                        "point the saved default at entry {ENTRY_OPENHC}. NOT a reboot: the box \
                         keeps running whatever it is running until you restart it"
                    ),
                ],
                writes: vec![
                    format!("{KERNEL_PART}: {KERNEL_FILE}, {INITRD_FILE} (files, on the spare kernel partition)"),
                    format!("{GRUB_PART}: /boot/grub/menu.lst and /boot/grub/default"),
                ],
                needs_serial: false,
                needs_button: false,
                reversible: format!(
                    "openHC re-points the default at the stock entry on every boot, so a reset of any kind already returns to Control4. To remove it entirely, delete the entry from menu.lst. {RESTORE_PART} is never written, and holding the ID button at power-on boots entry {ENTRY_FACTORY} regardless of what menu.lst says"
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
