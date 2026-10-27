//! The EM35x serial bootloader: its text menu, and an XMODEM-CRC upload.
//!
//! The standalone serial bootloader lives in a protected flash region that an
//! application upload never touches. It talks plain text at 115200 8N1:
//!
//! ```text
//! EM357 Serial Bootloader v43 b27
//! 1. upload ebl
//! 2. run
//! 3. ebl info
//! BL >
//! ```
//!
//! "1" starts an XMODEM-CRC receive of an `.ebl` image (128-byte blocks), "2"
//! runs the application. An upload that fails part-way leaves no valid app, so
//! the chip simply stays in the bootloader on its next reset — the flash can be
//! retried as many times as it takes.

use crate::ash::crc16;
use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

const SOH: u8 = 0x01;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const CAN: u8 = 0x18;
const PAD: u8 = 0x1a;
const CRC_MODE: u8 = b'C';
pub const PROMPT: &[u8] = b"BL >";

/// Read until `want` shows up (true) or `ms` passes (false). Returns what came.
pub fn read_until(fd: RawFd, ms: u64, want: &[u8]) -> (bool, Vec<u8>) {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut out = Vec::new();
    let mut b = [0u8; 256];
    while Instant::now() < end {
        let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, b.len()) };
        if n > 0 {
            out.extend_from_slice(&b[..n as usize]);
            if !want.is_empty() && out.windows(want.len()).any(|w| w == want) {
                return (true, out);
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    (false, out)
}

/// Poke the bootloader for its menu. True when it answers with the prompt.
pub fn at_prompt(fd: RawFd) -> bool {
    unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
    crate::write_all(fd, b"\r\n");
    let (ok, got) = read_until(fd, 2000, PROMPT);
    if crate::ash::debug() {
        eprintln!("btl < {got:02x?}");
    }
    ok
}

/// The bootloader's banner line ("EM357 Serial Bootloader v…"), for the log.
pub fn banner(fd: RawFd) -> Option<String> {
    unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
    crate::write_all(fd, b"\r\n");
    let (_, text) = read_until(fd, 2000, PROMPT);
    let s = String::from_utf8_lossy(&text);
    s.lines().find(|l| l.contains("Btl") || l.contains("Bootloader")).map(|l| l.trim().to_string())
}

/// One XMODEM-CRC block: SOH, block number and its complement, 128 bytes of
/// data (the last block padded with SUB), CRC-16/XMODEM big-endian.
pub fn block(n: u8, chunk: &[u8]) -> Vec<u8> {
    let mut data = [PAD; 128];
    data[..chunk.len()].copy_from_slice(chunk);
    let c = crc16(&data, 0);
    let mut p = vec![SOH, n, !n];
    p.extend_from_slice(&data);
    p.push((c >> 8) as u8);
    p.push(c as u8);
    p
}

/// Upload `image` through the bootloader's "1. upload ebl". The caller must
/// already be at the prompt. `progress` gets (blocks sent, blocks total).
pub fn upload(fd: RawFd, image: &[u8], mut progress: impl FnMut(usize, usize)) -> Result<(), String> {
    unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
    crate::write_all(fd, b"1");
    if !read_until(fd, 10_000, &[CRC_MODE]).0 {
        return Err("the bootloader never asked for an XMODEM-CRC upload".into());
    }
    let chunks: Vec<&[u8]> = image.chunks(128).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let pkt = block(((i + 1) & 0xff) as u8, chunk);
        let mut ok = false;
        for _try in 0..10 {
            unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
            crate::write_all(fd, &pkt);
            let (_, r) = read_until(fd, 3000, &[ACK]);
            if r.contains(&ACK) {
                ok = true;
                break;
            }
            if r.contains(&CAN) {
                return Err(format!("the bootloader cancelled the upload at block {}", i + 1));
            }
            // NAK or silence: send the block again.
        }
        if !ok {
            return Err(format!("block {} of {} was never acknowledged", i + 1, chunks.len()));
        }
        progress(i + 1, chunks.len());
    }
    for _try in 0..5 {
        crate::write_all(fd, &[EOT]);
        if read_until(fd, 3000, &[ACK]).0 {
            return Ok(());
        }
    }
    Err("the bootloader did not acknowledge the end of the upload".into())
}

/// "2. run": leave the bootloader for the application.
pub fn run_app(fd: RawFd) {
    crate::write_all(fd, b"2");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xmodem_block_layout() {
        let p = block(1, b"abc");
        assert_eq!(p.len(), 133);
        assert_eq!(&p[..3], &[SOH, 1, 0xfe]);
        assert_eq!(&p[3..6], b"abc");
        assert!(p[6..131].iter().all(|&b| b == PAD));
        let c = crc16(&p[3..131], 0);
        assert_eq!(&p[131..], &[(c >> 8) as u8, c as u8]);
        assert_eq!(block(255, &[0; 128])[2], 0);
    }
}
