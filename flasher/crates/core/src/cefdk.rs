//! CEFDK facts: eMMC layout, the kernel container, and the MFH autoscript.
//!
//! Pure constants and byte-builders, no I/O. Every number here was measured on
//! hardware — the comments say how, because a wrong value writes to the wrong
//! place on someone's flash.

// ------------------------------------------------------------- eMMC layout
//
// Measured on a live EA3 (matching the EA1). CEFDK's boot log states the first
// two outright:
//   "Read Kernel Size Successfully from emmc address(0x00000200)!"
//   "Successfully read (7009216) bytes of kernel ... from emmc address(0x00000400)"
//
//   0x000       MBR                          (0x55aa at 0x1fe)
//   0x200       container size, BE u32       (stock: 0x006af3c0 = 7,009,216)
//   0x400       CEFDK container header, 0x580 bytes
//   0x980       bzImage                      ("HdrS" at +0x202)
//   0x2000000   p1 begins (sector 65536, 32 MiB)

/// BE u32 total container length lives here.
pub const EMMC_SIZE_OFF: u64 = 0x200;
/// Container header starts here.
pub const EMMC_CONTAINER_OFF: u64 = 0x400;
/// Header length; the bzImage follows it.
pub const EMMC_HEADER_LEN: usize = 0x580;
/// Where the bzImage lands inside the container region.
pub const EMMC_KERNEL_OFF: u64 = EMMC_CONTAINER_OFF + EMMC_HEADER_LEN as u64;
/// First partition. Never write below this blindly.
pub const P1_START: u64 = 0x2000000;

/// Linux `/dev/mmcblk0` and CEFDK `emmc` agree on the offsets above — that is
/// why the network install works. They do NOT agree deep in the raw gap
/// (0xc00000), where a Linux-dd'd image reads back as garbage through CEFDK's
/// `emmc rd`. Keep raw-gap writes on the CEFDK side; keep container writes on
/// the Linux side. Proven the hard way.
pub const GAP_OFF_DEFAULT: u64 = 0xc00000;

pub const SECTOR: u64 = 512;

pub fn round_to_sector(n: u64) -> u64 {
    n.div_ceil(SECTOR) * SECTOR
}

// ------------------------------------------------------ MFH / autoscript
//
// `mfh list spi_nor` on a live EA3:
//   script   00 YES  0x00091000  0x00000800
//
// and the payload at that offset is plain ASCII — reading it back word by word
// gives "emmc", " rd ", "0xc0", ..., "200\0", then the next command begins right
// after the NUL. So the stored script is a run of NUL-terminated command
// strings: no header, no length prefix.
//
// THIS IS WHAT MAKES A NO-SERIAL TAKEOVER POSSIBLE on a secure-boot part: the
// region is `mtd0` from Linux, so the autoscript is writable over SSH, and the
// autoscript runs `bootlinux`, which does not verify the image.

/// SPI-NOR offset of the MFH `script` entry.
pub const MFH_SCRIPT_OFF: u64 = 0x91000;
/// Size of that entry.
pub const MFH_SCRIPT_LEN: usize = 0x800;

/// RAM address the kernel is staged at before `bootlinux`, and the two CEFDK
/// globals that say where the image is and that there is no ramdisk. Recovered
/// from the working takeover; not derivable from anything public.
pub const KERNEL_ADDR: u64 = 0x6000000;
/// RAM address the initramfs is staged at (the `tftp get`/`emmc rd` destination
/// for rootfs.cpio.gz). Well clear of the kernel at 0x6000000.
pub const RAMDISK_ADDR: u64 = 0x4000000;
pub const G_KBASE: u64 = 0xc90a4; // ord4 <this> = KERNEL_ADDR
pub const G_RD_FLAG: u64 = 0x837560; // ord4 <this> = 0 -> no ramdisk, 1 -> ramdisk present
pub const G_RD_ADDR: u64 = 0x837564; // ord4 <this> = RAMDISK_ADDR (when G_RD_FLAG = 1)
pub const G_RD_SIZE: u64 = 0x837568; // ord4 <this> = initramfs length in bytes

pub const DEFAULT_CMDLINE: &str =
    "console=ttyS0,115200 pci=realloc,nocrs root=/dev/mmcblk0p1 rootwait rw";

/// Command line for a pure-RAM boot off an initramfs: no `root=`, the rootfs IS
/// the ramdisk. `routeirq` is carried because the EA's SoC UARTs need it, and
/// `nocrs` because CEFDK's E820 does not describe the PCI host-bridge windows.
pub const RAMBOOT_CMDLINE: &str =
    "console=ttyS0,115200 pci=realloc,nocrs,routeirq rw";

/// The five commands that boot our kernel from raw eMMC.
///
/// `cache flush` is not optional: CEFDK's `emmc rd` DMAs into RAM without
/// invalidating the CPU cache, so without it bootlinux parses stale memory and
/// dies. That one line cost a full debugging session.
pub fn autoscript_for(kernel_off: u64, kernel_len: u64, cmdline: &str) -> Vec<String> {
    let size = round_to_sector(kernel_len);
    vec![
        format!("emmc rd {kernel_off:#x} {KERNEL_ADDR:#x} {size:#x}"),
        "cache flush".into(),
        format!("ord4 {G_KBASE:#x} = {KERNEL_ADDR:#x}"),
        format!("ord4 {G_RD_FLAG:#x} = 0x0"),
        format!("bootlinux \"{cmdline}\""),
    ]
}

/// The CEFDK shell commands that netboot openHC into RAM: pull the kernel and
/// initramfs over TFTP, then boot the initramfs directly (no `root=`).
///
/// This is the sibling of [`autoscript_for`] for the bring-up loop: same RAM
/// staging and same `ord4` globals, but the images come from the network at the
/// unlocked manufacturing shell instead of from eMMC, and there is a ramdisk, so
/// `G_RD_FLAG` is 1 and `G_RD_ADDR`/`G_RD_SIZE` describe it. The size is the
/// exact initramfs byte count: bootlinux hands it to the kernel as the ramdisk
/// length, and the kernel's gzip/cpio reader needs the whole image (unlike a
/// bzImage, it has no self-describing end the loader can find).
///
/// `box_ip` is the address CEFDK should take for the transfer (the BOOTP offer);
/// the mask/gateway match the point-to-point bring-up link. `cache flush` is as
/// mandatory here as in [`autoscript_for`] — the TFTP load DMAs into RAM without
/// invalidating the CPU cache.
pub fn ramboot_tftp_for(
    server_ip: &str,
    box_ip: &str,
    kernel_name: &str,
    initrd_name: &str,
    initrd_len: u64,
    cmdline: &str,
) -> Vec<String> {
    vec![
        format!("ip set {box_ip} 255.255.255.0 0.0.0.0"),
        format!("tftp get {server_ip} {KERNEL_ADDR:#x} {kernel_name}"),
        format!("tftp get {server_ip} {RAMDISK_ADDR:#x} {initrd_name}"),
        "cache flush".into(),
        format!("ord4 {G_KBASE:#x} = {KERNEL_ADDR:#x}"),
        format!("ord4 {G_RD_FLAG:#x} = 0x1"),
        format!("ord4 {G_RD_ADDR:#x} = {RAMDISK_ADDR:#x}"),
        format!("ord4 {G_RD_SIZE:#x} = {initrd_len:#x}"),
        format!("bootlinux \"{cmdline}\""),
    ]
}

/// Pack shell commands into the MFH `script` payload: NUL-terminated strings
/// back to back, a final NUL to end the list, then 0xff padding (erased-flash
/// value, so a shorter re-write does not disturb what follows).
pub fn build_autoscript(lines: &[String]) -> Result<Vec<u8>, String> {
    let mut blob = Vec::new();
    for l in lines {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        blob.extend_from_slice(l.as_bytes());
        blob.push(0);
    }
    blob.push(0);
    if blob.len() > MFH_SCRIPT_LEN {
        return Err(format!(
            "autoscript is {} bytes, MFH script entry holds {MFH_SCRIPT_LEN}",
            blob.len()
        ));
    }
    blob.resize(MFH_SCRIPT_LEN, 0xff);
    Ok(blob)
}

/// Inverse of [`build_autoscript`], for showing what a unit currently has.
pub fn parse_autoscript(blob: &[u8]) -> Vec<String> {
    let mut out = vec![];
    for chunk in blob.split(|&b| b == 0) {
        let chunk: Vec<u8> = chunk.iter().copied().filter(|&b| b != 0xff).collect();
        if chunk.is_empty() {
            break;
        }
        out.push(String::from_utf8_lossy(&chunk).into_owned());
    }
    out
}
