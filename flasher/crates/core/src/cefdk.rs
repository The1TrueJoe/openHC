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
pub const G_KBASE: u64 = 0xc90a4; // ord4 <this> = KERNEL_ADDR
pub const G_RD_FLAG: u64 = 0x837560; // ord4 <this> = 0 -> no ramdisk

pub const DEFAULT_CMDLINE: &str =
    "console=ttyS0,115200 pci=realloc,nocrs root=/dev/mmcblk0p1 rootwait rw";

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
