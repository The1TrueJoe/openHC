//! A synchronous request/response link to the IO microcontroller.
//!
//! Deliberately blocking, and deliberately behind one mutex. The MCU is a
//! single UART that answers one question at a time; modelling it as anything
//! richer than "take the lock, ask, wait for the matching seq, release" buys
//! nothing and makes interleaving bugs possible. The async layer above calls
//! this from `spawn_blocking`.
use crate::mcu::{self, Decoder, Frame};
use std::io;
use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

pub struct Link {
    fd: RawFd,
    dec: Decoder,
    seq: u8,
    pub part: String,
    pub baud: u32,
}

impl Drop for Link {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

impl Link {
    pub fn open(dev: &str, baud: u32, part: &str) -> io::Result<Link> {
        Ok(Link { fd: mcu::open_serial(dev, baud)?, dec: Decoder::default(), seq: 0, part: part.into(), baud })
    }

    fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut off = 0;
        while off < buf.len() {
            let n = unsafe { libc::write(self.fd, buf[off..].as_ptr() as *const _, buf.len() - off) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                return Err(e);
            }
            off += n as usize;
        }
        Ok(())
    }

    /// Send `opcode` and wait for its reply.
    ///
    /// The match is on SEQ, not opcode, because the reply opcode is not always
    /// request+1 — 0xD2 answers 0xD7. Sequence is the only field that is
    /// reliably echoed, so it is the only safe correlator.
    pub fn request(&mut self, opcode: u8, payload: &[u8], timeout: Duration) -> io::Result<Frame> {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        self.write_all(&mcu::encode(opcode, seq, 0, payload))?;

        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 512];
        let mut found: Option<Frame> = None;
        while Instant::now() < deadline && found.is_none() {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n > 0 {
                let dec = &mut self.dec;
                dec.feed(&buf[..n as usize], |f| {
                    if f.seq == seq && found.is_none() {
                        found = Some(f);
                    }
                });
            } else {
                std::thread::sleep(Duration::from_millis(3));
            }
        }
        found.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no reply from the IO MCU"))
    }

    /// Fire and forget — used where the firmware sends no reply.
    pub fn send(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        self.seq = self.seq.wrapping_add(1);
        let s = self.seq;
        self.write_all(&mcu::encode(opcode, s, 0, payload))
    }

    pub fn identify(&mut self) -> io::Result<(String, String)> {
        let t = Duration::from_millis(500);
        let ver = self.request(mcu::OP_FIRMWARE_VERSION, &[], t)?;
        let name = self.request(mcu::OP_PRODUCT_NAME, &[], t)?;
        Ok((
            String::from_utf8_lossy(&name.payload).to_string(),
            String::from_utf8_lossy(&ver.payload).to_string(),
        ))
    }

    /// u32 big-endian bitmask, bit N = contact N. A CLOSED contact reads 1 —
    /// established on a live EA3 by shorting the input, and confirmed on an
    /// HC-800 which answers all-zero with nothing connected.
    pub fn contacts(&mut self) -> io::Result<u32> {
        let f = self.request(mcu::OP_CONTACT_GET, &[], Duration::from_millis(400))?;
        let p = &f.payload;
        if p.len() < 4 {
            return Err(io::Error::other("short CONTACT_STATE"));
        }
        Ok(u32::from_be_bytes([p[0], p[1], p[2], p[3]]))
    }

    /// Raw RELAY_STATE payload.
    ///
    /// Returned raw ON PURPOSE. A live HC-800 with four relays answers `ff 00`,
    /// which cannot be a "bit N = relay N energised" map — that would read 0x0f
    /// at most. Active-low, a present/valid mask and an uninitialised sentinel
    /// are all still open. Handing the caller the bytes is honest; inventing an
    /// interpretation here would bake a guess into every consumer.
    pub fn relays_raw(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.request(mcu::OP_RELAY_GET, &[], Duration::from_millis(400))?.payload)
    }

    pub fn relay_toggle(&mut self, mask: u8) -> io::Result<Vec<u8>> {
        Ok(self.request(mcu::OP_RELAY_TOGGLE, &[mask], Duration::from_millis(600))?.payload)
    }

    /// Measured baud the MCU reports (BE32). Useful as a link sanity check.
    pub fn measured_baud(&mut self) -> io::Result<u32> {
        let f = self.request(mcu::OP_AUTO_BAUD, &[], Duration::from_millis(400))?;
        let p = &f.payload;
        if p.len() < 4 {
            return Err(io::Error::other("short AUTO_BAUD_RESP"));
        }
        Ok(u32::from_be_bytes([p[0], p[1], p[2], p[3]]))
    }
}
