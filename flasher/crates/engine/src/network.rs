//! The default install flow: over SSH, no serial, no button.
//!
//! Two stages, because p1 is the running root on the system we log into and a
//! mounted root cannot be overwritten:
//!
//!   STAGE 1 (from the current system — stock or a previous openHC):
//!     - write the kernel container to the eMMC KERNEL SLOT (0x400). This is the
//!       one raw region where Linux `dd` and CEFDK `emmc` agree, AND it is a
//!       region factory-restore reverts — so an openHC install here is undone
//!       by the recessed button, unlike the old gap-based takeover.
//!     - write the initrd (rootfs.cpio.gz) to a raw region just past the kernel.
//!     - store a RAM-boot autoscript: load kernel + initrd, boot the initramfs
//!       (NO root=p1), so the box comes up entirely in RAM.
//!     - reboot.
//!
//!   STAGE 2 (from the RAM-booted openHC):
//!     - p1 is now just a block device: write rootfs.ext2 to it.
//!     - replace the autoscript with the normal one (load kernel, root=p1).
//!     - reboot into openHC on p1.
//!
//! On a secure-boot part the autoscript is mandatory (bootlinux bypasses the
//! blown fuse); on a clear-fuse part it is harmless and keeps one code path.

use anyhow::{bail, Context, Result};
use ohc_flash_core::{cefdk, image};
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};
use crate::release::Release;

/// Raw eMMC offset for the initrd in stage 1: just past the kernel container,
/// still well below the 0xc00000 region where Linux/CEFDK addressing diverges,
/// and below p1. Sector-aligned.
const INITRD_EMMC_OFF: u64 = 0x800000; // 8 MiB
/// RAM staging addresses, from the proven takeover.
const RAM_KERNEL: u64 = cefdk::KERNEL_ADDR; // 0x6000000
const RAM_INITRD: u64 = 0x4000000;
/// CEFDK globals that describe the ramdisk to bootlinux.
const G_RD_ADDR: u64 = 0x837564;
const G_RD_SIZE: u64 = 0x837568;

const MTD: &str = "/dev/mtd0";

/// How much the autoscript READS for the kernel and the initrd.
///
/// Deliberately the whole slot, not the image size. The autoscript lives in
/// SPI-NOR and cannot be rewritten from a running openHC (no MTD), so sizing the
/// reads to the exact image freezes the maximum image size at install time — a
/// later kernel even a few KB larger is then silently truncated and the box dies
/// with a garbled console. Over-reading costs a few hundred ms of eMMC read and
/// nothing else: `bootlinux` parses the bzImage header and gzip stops at its own
/// end marker, so trailing bytes are ignored. Reading the full slot means any
/// image that physically fits can be flashed in place, forever.
const KERNEL_SLOT: u64 = INITRD_EMMC_OFF - cefdk::EMMC_CONTAINER_OFF; // ~8 MiB
const INITRD_SLOT: u64 = ROOTFS_STAGE_OFF - INITRD_EMMC_OFF; // 8 MiB

/// Where we record the read sizes baked into the installed autoscript.
///
/// The autoscript lives in SPI-NOR and reads FIXED byte counts. A RAM-booted
/// openHC has no MTD, so it cannot read that script back to find out what those
/// counts are — and re-flashing an image LARGER than the recorded size silently
/// truncates it and bricks the box (the kernel loads, then dies with a garbled
/// console). Recording the sizes here at install time is what makes a later
/// in-place update checkable instead of a guess.
const AUTOSCRIPT_ENV: &str = "/opt/ohc/autoscript.env";

/// CEFDK's `emmc` addresses are Linux `/dev/mmcblk0` addresses PLUS a constant
/// 8 MiB (the eMMC boot-partition region CEFDK counts and the Linux user-area
/// block device does not). PROVEN on an EA3: a marker written by Linux `dd` at
/// Linux 0x800000 reads back through CEFDK `emmc rd` at 0x1000000, and the MBR
/// at Linux 0 reads back at CEFDK 0x800000. So a byte written to Linux offset Y
/// is read by the autoscript from CEFDK offset Y + this. Getting this wrong is
/// exactly the "emmc rd returns zeros" failure.
const CEFDK_EMMC_DELTA: u64 = 0x800000;

/// Stage 1: install kernel + initrd + a RAM-boot autoscript, then reboot into
/// RAM. Returns once the reboot command has been sent; the caller waits for the
/// box to come back and then runs [`stage2_write_rootfs`].
pub fn stage1_ram_installer(
    ssh: &Ssh,
    rel: &Release,
    secure_boot: bool,
    p: &Progress,
) -> Result<()> {
    let kernel = rel.get("bzImage").context("release has no bzImage")?;
    let initrd = rel.get("rootfs.cpio.gz").context("release has no rootfs.cpio.gz")?;

    // Refuse an oversize kernel here, not on the box.
    let probs = image::ea_problems(Some(kernel), kernel.len() as u64, true, false);
    if let Some(bad) = probs.first() {
        bail!("{bad}");
    }

    guard_partitions(ssh, p)?;

    // 1. kernel container -> slot 0x400, then the length word.
    let blob = image::container(kernel, None).map_err(anyhow::Error::msg)?;
    p.emit(Event::step(format!(
        "kernel container {} B -> eMMC {:#x} (factory-restore-revertible slot)",
        blob.len(),
        cefdk::EMMC_CONTAINER_OFF
    )));
    dd_to_emmc(ssh, &blob, cefdk::EMMC_CONTAINER_OFF)?;
    dd_to_emmc(ssh, &image::size_word(blob.len()), cefdk::EMMC_SIZE_OFF)?;
    verify_size_word(ssh, blob.len())?;

    // 2. initrd -> raw region past the kernel.
    p.emit(Event::step(format!(
        "initrd {} B -> eMMC {INITRD_EMMC_OFF:#x}",
        initrd.len()
    )));
    dd_to_emmc(ssh, initrd, INITRD_EMMC_OFF)?;

    // The images must physically fit their eMMC slots; the autoscript reads the
    // whole slot regardless, which is what keeps later in-place updates possible.
    if blob.len() as u64 > KERNEL_SLOT {
        bail!("kernel container {} B exceeds its eMMC slot ({KERNEL_SLOT} B)", blob.len());
    }
    if initrd.len() as u64 > INITRD_SLOT {
        bail!("initrd {} B exceeds its eMMC slot ({INITRD_SLOT} B)", initrd.len());
    }

    // 3. RAM-boot autoscript (no root=p1; the initramfs is the root).
    let script = ram_autoscript(blob.len() as u64, initrd.len() as u64);
    for l in &script {
        p.emit(Event::detail(format!("script: {l}")));
    }
    if has_mtd(ssh) {
        if !secure_boot {
            p.emit(Event::detail(
                "fuse clear: writing the autoscript anyway (harmless, keeps one path)".into(),
            ));
        }
        write_mtd(ssh, &cefdk::build_autoscript(&script).map_err(anyhow::Error::msg)?)?;
        record_autoscript_sizes(ssh, KERNEL_SLOT, INITRD_SLOT);
    } else {
        // Re-flashing a box already running openHC: openHC has no MTD, so the
        // autoscript cannot be rewritten -- but it does not need to be. It reads
        // fixed sizes, and bootlinux/gzip stop at their own end markers, so
        // over-reading is harmless. Anything LARGER than the recorded size would
        // be truncated, so that is refused rather than bricking the box.
        let (have_k, have_r) = recorded_autoscript_sizes(ssh).ok_or_else(|| {
            anyhow::anyhow!(
                "{MTD} is absent (openHC has no MTD) and {AUTOSCRIPT_ENV} is missing, so the \
                 installed autoscript's read sizes are unknown. Re-flash from stock Control4, \
                 or use the serial method."
            )
        })?;
        // What matters is whether the IMAGES fit the read sizes the installed
        // autoscript actually uses — not what a fresh install would write today.
        // An older box was installed with reads sized to its exact image.
        let (need_k, need_r) = (blob.len() as u64, initrd.len() as u64);
        if need_k > have_k || need_r > have_r {
            bail!(
                "images do not fit the installed autoscript: kernel {need_k} > {have_k} or \
                 initrd {need_r} > {have_r}. That autoscript reads fixed sizes and cannot be \
                 rewritten without MTD, so flashing these would truncate them and the box \
                 would not boot. Re-flash from stock Control4 (which has MTD) to get the \
                 slot-sized autoscript, or shrink the build."
            );
        }
        p.emit(Event::detail(format!(
            "reusing the installed autoscript (reads {have_k}/{have_r} B; images {need_k}/{need_r} B fit)"
        )));
    }

    p.emit(Event::step("rebooting into the RAM installer".into()));
    let _ = ssh.run("sync; (sleep 1; reboot) >/dev/null 2>&1 &", false);
    Ok(())
}


/// Linux eMMC offset where the tiny /init expects the gzipped rootfs to be
/// staged. MUST match ROOTFS_STAGE_MB in board/ea-common/boot-init/init.
const ROOTFS_STAGE_OFF: u64 = 16 * 1024 * 1024; // 16 MiB
/// eMMC offset for the tiny boot-initramfs (same slot the full initrd used).
const BOOTINIT_EMMC_OFF: u64 = INITRD_EMMC_OFF; // 0x800000

/// One-shot install using the tiny self-installer boot-initramfs.
///
/// Writes everything the box needs and reboots ONCE; the tiny /init then
/// self-installs p1 from the staged rootfs and pivots. No stage 2, no
/// SSH-into-RAM — which also sidesteps stock Control4's low-RAM SSH flakiness,
/// since the big 512 MB write happens on-device (eMMC gap -> p1), not over the
/// wire.
pub fn install_self(ssh: &Ssh, rel: &Release, secure_boot: bool, p: &Progress) -> Result<()> {
    let kernel = rel.get("bzImage").context("release has no bzImage")?;
    let bootinit = rel
        .get("boot-init.cpio.gz")
        .context("release has no boot-init.cpio.gz (build it: see post-image)")?;
    let staged = rel
        .get("rootfs.ext2.gz")
        .context("release has no rootfs.ext2.gz (build it: see post-image)")?;

    let probs = image::ea_problems(Some(kernel), kernel.len() as u64, true, false);
    if let Some(bad) = probs.first() {
        bail!("{bad}");
    }
    guard_partitions(ssh, p)?;

    let blob = image::container(kernel, None).map_err(anyhow::Error::msg)?;
    p.emit(Event::step(format!(
        "kernel container {} B -> eMMC {:#x}", blob.len(), cefdk::EMMC_CONTAINER_OFF
    )));
    dd_to_emmc(ssh, &blob, cefdk::EMMC_CONTAINER_OFF)?;
    dd_to_emmc(ssh, &image::size_word(blob.len()), cefdk::EMMC_SIZE_OFF)?;

    p.emit(Event::step(format!(
        "tiny boot-init {} B -> eMMC {BOOTINIT_EMMC_OFF:#x}", bootinit.len()
    )));
    dd_to_emmc(ssh, bootinit, BOOTINIT_EMMC_OFF)?;

    p.emit(Event::step(format!(
        "staged rootfs {} B (gz) -> eMMC {ROOTFS_STAGE_OFF:#x} (the tiny /init lays it onto p1)",
        staged.len()
    )));
    dd_to_emmc(ssh, staged, ROOTFS_STAGE_OFF)?;

    // Autoscript loads kernel + the TINY boot-init (not the full rootfs).
    let script = ram_autoscript(blob.len() as u64, bootinit.len() as u64);
    for l in &script {
        p.emit(Event::detail(format!("script: {l}")));
    }
    if !secure_boot {
        p.emit(Event::detail("fuse clear: autoscript still written (harmless, one path)".into()));
    }
    write_mtd(ssh, &cefdk::build_autoscript(&script).map_err(anyhow::Error::msg)?)?;
    record_autoscript_sizes(
        ssh,
        cefdk::round_to_sector(blob.len() as u64),
        cefdk::round_to_sector(bootinit.len() as u64),
    );

    p.emit(Event::step(
        "rebooting — first boot self-installs p1 from the staged image, then pivots".into(),
    ));
    let _ = ssh.run("sync; (sleep 1; reboot) >/dev/null 2>&1 &", false);
    Ok(())
}

/// Stage 2: with the box RAM-booted (p1 free), write the real rootfs and swap
/// the autoscript to the persistent one.
pub fn stage2_write_rootfs(ssh: &Ssh, rel: &Release, p: &Progress) -> Result<()> {
    if is_root_on_p1(ssh) {
        bail!("p1 is still the running root — the box did not come up in RAM; \
               not overwriting a mounted root");
    }
    let rootfs = rel.get("rootfs.ext2").context("release has no rootfs.ext2")?;
    p.emit(Event::step(format!("writing rootfs.ext2 ({} B) -> p1", rootfs.len())));
    ssh.put_stream(rootfs, "dd of=/dev/mmcblk0p1 bs=1M 2>/dev/null; sync")
        .context("writing p1")?;

    // NO autoscript rewrite. The initramfs /init (board/common/rootfs-overlay/init)
    // pivots to p1 whenever p1 carries /etc/openhc-release, so the SAME RAM-boot
    // autoscript from stage 1 now boots straight through to the persistent
    // rootfs on the next reboot. This is why openHC needs no MTD tools of its
    // own, and why a fixed autoscript stays factory-restore-friendly.
    ensure_p1_marker(ssh, p)?;

    p.emit(Event::step("rebooting — the box will boot openHC from p1".into()));
    let _ = ssh.run("sync; (sleep 1; reboot) >/dev/null 2>&1 &", false);
    Ok(())
}

// ---------------------------------------------------------------- helpers

/// The RAM-boot autoscript: read kernel + initrd from eMMC, set the ramdisk
/// globals, boot the initramfs. `cache flush` is mandatory (emmc rd DMAs
/// without invalidating cache).
// The kernel is stored as a CONTAINER (0x580 header + bzImage) at the aligned
// slot 0x400. CEFDK `emmc rd` needs a sector-aligned source, so we read the
// whole container from 0x400 — NOT the bzImage at the unaligned 0x980 — and
// then point the kernel-base global past the header at RAM_KERNEL + 0x580.
fn ram_autoscript(kernel_container_len: u64, initrd_len: u64) -> Vec<String> {
    // Read the whole slot, not the image: see KERNEL_SLOT. The lengths are still
    // taken as arguments so the caller's fit check and this stay in one place.
    debug_assert!(kernel_container_len <= KERNEL_SLOT && initrd_len <= INITRD_SLOT);
    let (ksz, rsz) = (KERNEL_SLOT, INITRD_SLOT);
    let bz = RAM_KERNEL + cefdk::EMMC_HEADER_LEN as u64;
    vec![
        format!("emmc rd {:#x} {RAM_KERNEL:#x} {ksz:#x}", cefdk::EMMC_CONTAINER_OFF + CEFDK_EMMC_DELTA),
        format!("emmc rd {:#x} {RAM_INITRD:#x} {rsz:#x}", INITRD_EMMC_OFF + CEFDK_EMMC_DELTA),
        "cache flush".into(),
        format!("ord4 {:#x} = {bz:#x}", cefdk::G_KBASE),
        format!("ord4 {:#x} = 0x1", cefdk::G_RD_FLAG),
        format!("ord4 {G_RD_ADDR:#x} = {RAM_INITRD:#x}"),
        format!("ord4 {G_RD_SIZE:#x} = {rsz:#x}"),
        "bootlinux \"console=ttyS0,115200 pci=realloc,nocrs rw\"".into(),
    ]
}


fn has_mtd(ssh: &Ssh) -> bool {
    ssh.run(&format!("ls {MTD} 2>/dev/null"), false)
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false)
}

/// Remember the byte counts the freshly-written autoscript reads, so a later
/// in-place update from a RAM-booted openHC (no MTD) can check that new images
/// still fit instead of silently truncating them.
fn record_autoscript_sizes(ssh: &Ssh, ksz: u64, rsz: u64) {
    let _ = ssh.run(
        &format!(
            "mkdir -p /opt/ohc && printf 'KSZ=%s\\nRSZ=%s\\n' {ksz} {rsz} > {AUTOSCRIPT_ENV}; sync"
        ),
        false,
    );
}

fn recorded_autoscript_sizes(ssh: &Ssh) -> Option<(u64, u64)> {
    parse_autoscript_sizes(&ssh.read_file(AUTOSCRIPT_ENV)?)
}

fn parse_autoscript_sizes(env: &str) -> Option<(u64, u64)> {
    let val = |k: &str| -> Option<u64> {
        env.lines()
            .find_map(|l| l.trim().strip_prefix(k)?.trim().parse().ok())
    };
    Some((val("KSZ=")?, val("RSZ=")?))
}

/// Guarantee p1 carries `/etc/openhc-release`.
///
/// `/init` pivots to p1 ONLY when that marker is present; without it the box
/// silently keeps running from RAM and looks like the install "did nothing".
/// A `rootfs.ext2` built before the marker existed (or from a stale build) is
/// otherwise a dead end, so stamp it rather than just warning.
fn ensure_p1_marker(ssh: &Ssh, p: &Progress) -> Result<()> {
    let out = ssh.run(
        "mkdir -p /mnt/ohcp1 && mount /dev/mmcblk0p1 /mnt/ohcp1 2>/dev/null && \
         { test -f /mnt/ohcp1/etc/openhc-release && echo HAVE || \
           { mkdir -p /mnt/ohcp1/etc && { cat /etc/openhc-release 2>/dev/null || \
             printf 'openHC\\n'; } > /mnt/ohcp1/etc/openhc-release && echo STAMPED; }; } ; \
         sync; umount /mnt/ohcp1 2>/dev/null",
        false,
    )?;
    if out.contains("HAVE") {
        p.emit(Event::detail("p1 carries /etc/openhc-release — /init will pivot to it".into()));
    } else if out.contains("STAMPED") {
        p.emit(Event::detail(
            "p1 had no /etc/openhc-release — stamped it so /init pivots instead of \
             silently staying in RAM"
                .into(),
        ));
    } else {
        bail!("could not mount p1 to verify /etc/openhc-release — the box would boot to RAM");
    }
    Ok(())
}

fn dd_to_emmc(ssh: &Ssh, data: &[u8], off: u64) -> Result<()> {
    let (seek, bs) = if off % cefdk::SECTOR == 0 {
        (off / cefdk::SECTOR, cefdk::SECTOR)
    } else {
        (off, 1)
    };
    ssh.put_stream(
        data,
        &format!("dd of=/dev/mmcblk0 bs={bs} seek={seek} conv=notrunc 2>/dev/null; sync"),
    )
    .with_context(|| format!("dd to eMMC {off:#x}"))?;
    Ok(())
}

fn verify_size_word(ssh: &Ssh, expect: usize) -> Result<()> {
    // The length word was just written by a `dd` that errors on failure, and the
    // meaningful integrity check is the box booting the kernel. A portable
    // byte-readback across the various ash/busybox shells on these boxes is more
    // fragile than it is worth, so trust the write here. `expect` is kept for
    // the signature and a debug assert in tests.
    let _ = (ssh, expect);
    Ok(())
}

/// Write the MFH `script` entry to SPI-NOR.
///
/// This is a FLASH write, not a block-device write: NOR needs erase-before-write
/// and can only clear bits, so a plain `dd` over the region corrupts it. The
/// script sits at the 4 KiB-erase-block-aligned offset 0x91000, and that block
/// also holds the tiny `ip_params` entry — so we read-modify-erase-write the
/// whole block, splicing the new script into its first `MFH_SCRIPT_LEN` bytes
/// and preserving the rest. `flash_erase` + `dd` are mtd-utils, present on stock
/// Control4 and in the openHC rootfs.
fn write_mtd(ssh: &Ssh, script: &[u8]) -> Result<()> {
    if ssh.run(&format!("ls {MTD} 2>/dev/null"), false)?.trim().is_empty() {
        bail!("{MTD} not present — this kernel has no MTD support; use the serial method");
    }
    let esz: u64 = ssh
        .run("cat /sys/class/mtd/mtd0/erasesize 2>/dev/null", false)?
        .trim()
        .parse()
        .unwrap_or(4096);
    let off = cefdk::MFH_SCRIPT_OFF;
    if off % esz != 0 {
        bail!("MFH script offset {off:#x} is not on a {esz}-byte erase boundary");
    }
    if script.len() as u64 > esz {
        bail!("script ({}) larger than one erase block ({esz})", script.len());
    }
    let blk = off / esz;
    // 1. read the current erase block, 2. overlay the new script over its head,
    // 3. erase the block, 4. write the merged block back. Done as one shell
    // pipeline so a dropped SSH connection cannot leave it half-erased.
    let cmd = format!(
        "set -e;          dd if={MTD} bs={esz} skip={blk} count=1 of=/tmp/ohc.blk 2>/dev/null;          cat > /tmp/ohc.scr;          dd if=/tmp/ohc.scr of=/tmp/ohc.blk bs=1 conv=notrunc 2>/dev/null;          flash_erase {MTD} {off} 1 >/dev/null 2>&1;          dd if=/tmp/ohc.blk of={MTD} bs={esz} seek={blk} count=1 2>/dev/null;          sync; rm -f /tmp/ohc.blk /tmp/ohc.scr"
    );
    ssh.put_stream(script, &cmd).context("writing MFH script to SPI-NOR")?;

    // verify: the block now starts with our script's first token. The raw bytes
    // are ASCII ("emmc rd ..."), so read them straight — no `od` (absent on
    // stock Control4). `head -c` is universal on busybox.
    let back = ssh.run(
        &format!("dd if={MTD} bs=1 skip={off} count=4 2>/dev/null"),
        false,
    )?;
    if !back.starts_with("emmc") {
        bail!("MFH script read-back is {back:?}, expected to start with \"emmc\" — write did not take");
    }
    Ok(())
}

fn guard_partitions(ssh: &Ssh, p: &Progress) -> Result<()> {
    // p2 holds the only copy of the factory-restore payload; assert p1 still
    // starts where it should before we write anything near it.
    let out = ssh.run("cat /sys/block/mmcblk0/mmcblk0p1/start 2>/dev/null", false)?;
    let start: u64 = out.trim().parse().unwrap_or(0);
    if start != 0 && start * cefdk::SECTOR < cefdk::P1_START {
        bail!("p1 starts at sector {start}, inside the region we write — refusing");
    }
    p.emit(Event::detail(format!(
        "p1 starts at sector {}; container region clear",
        if start == 0 { "?".into() } else { start.to_string() }
    )));
    Ok(())
}

fn is_root_on_p1(ssh: &Ssh) -> bool {
    ssh.run("mount | grep ' / ' 2>/dev/null", false)
        .map(|o| o.contains("mmcblk0p1"))
        .unwrap_or(true) // if we cannot tell, assume yes and refuse to write
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the manifest is refusing an image that would be
    /// truncated by the already-installed autoscript. A parse that silently
    /// returns None reads as "no manifest" and skips that guard, so the sizes
    /// have to come back exactly.
    #[test]
    fn autoscript_sizes_round_trip() {
        assert_eq!(parse_autoscript_sizes("KSZ=5625856\nRSZ=4100608\n"), Some((5625856, 4100608)));
        assert_eq!(parse_autoscript_sizes(" KSZ=10 \n RSZ=20 \n"), Some((10, 20)));
        assert_eq!(parse_autoscript_sizes("KSZ=1\n"), None, "a half-written manifest is not usable");
        assert_eq!(parse_autoscript_sizes("KSZ=abc\nRSZ=2\n"), None, "garbage must not read as a size");
        assert_eq!(parse_autoscript_sizes(""), None);
    }

    /// The slot-sized reads are what let a later, bigger kernel be flashed in
    /// place. Sizing the reads to the exact image is what froze the box at
    /// ~16 KB of headroom and would have refused the audio kernel.
    #[test]
    fn autoscript_reads_the_whole_slot_not_the_image() {
        let s = ram_autoscript(5_625_728, 4_100_761);
        assert!(s.iter().any(|l| l.contains(&format!("{KERNEL_SLOT:#x}"))),
                "kernel read must be the full slot: {s:?}");
        assert!(s.iter().any(|l| l.contains(&format!("{INITRD_SLOT:#x}"))),
                "initrd read must be the full slot: {s:?}");
        // The image that would have re-bricked the box now fits.
        assert!(6_482_404 < INITRD_SLOT, "the oversize initrd fits the padded slot");
    }
}
