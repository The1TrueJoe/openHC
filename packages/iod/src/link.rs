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
    /// Frames the MCU sent that nobody asked for — an IR capture arriving
    /// because somebody pressed a remote. `request` correlates on SEQ and would
    /// otherwise drop these on the floor, which is precisely the traffic an
    /// external control system most wants to see.
    stray: Vec<Frame>,
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
        Ok(Link {
            fd: mcu::open_serial(dev, baud)?,
            dec: Decoder::default(),
            seq: 0,
            stray: Vec::new(),
            part: part.into(),
            baud,
        })
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
                let stray = &mut self.stray;
                dec.feed(&buf[..n as usize], |f| {
                    if f.seq == seq && found.is_none() {
                        found = Some(f);
                    } else {
                        // Not our reply. Keep it: this is how an IR capture,
                        // which the MCU sends whenever a remote is pressed,
                        // reaches the event bus.
                        stray.push(f);
                    }
                });
            } else {
                std::thread::sleep(Duration::from_millis(3));
            }
        }
        found.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no reply from the IO MCU"))
    }

    /// Read for `dur`, collecting frames the MCU sent on its own.
    ///
    /// The contact poller calls this so unsolicited traffic is picked up even
    /// during idle periods, rather than sitting in the tty buffer until the
    /// next request happens to read it.
    pub fn poll(&mut self, dur: Duration) -> Vec<Frame> {
        let deadline = Instant::now() + dur;
        let mut buf = [0u8; 512];
        while Instant::now() < deadline {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n > 0 {
                let mut got = Vec::new();
                self.dec.feed(&buf[..n as usize], |f| got.push(f));
                self.stray.extend(got);
            } else {
                break;
            }
        }
        std::mem::take(&mut self.stray)
    }

    /// Everything unsolicited seen so far, cleared by the read.
    pub fn take_stray(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.stray)
    }

    /// Ask the MCU to report IR it receives.
    ///
    /// Without this the receiver is deaf and no `ir.rx` event can ever fire.
    ///
    /// UNVERIFIED: the opcode is the vendor's (0x77 IRIN_SET_CAPTURE) but the
    /// one-byte on/off payload is inferred from the name, not observed. The
    /// firmware sends no reply, so a wrong payload fails silently — if pressing
    /// a remote produces no `ir/rx` event, this argument is the first suspect.
    pub fn ir_capture(&mut self, on: bool) -> io::Result<()> {
        self.send(mcu::OP_IRIN_SET_CAPTURE, &[if on { 1 } else { 0 }])
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

    /// State of ONE relay, addressed by 0-BASED INDEX.
    ///
    /// The selector is an index, not a bitmask. The firmware echoes whatever it
    /// is handed — 0x00 through 0x09 all come back verbatim — so a GET cannot
    /// distinguish the two readings. A TOGGLE sweep can, and did: on a live
    /// HC-800, selectors 0x00..0x03 each flip exactly one relay while 0x04 and
    /// above do nothing.
    ///
    ///     TOGGLE 00 -> relay 0 moves      TOGGLE 04 -> nothing
    ///     TOGGLE 03 -> relay 3 moves      TOGGLE 08 -> nothing
    ///
    /// This was previously read as a bitmask, which sent 1<<index and so
    /// addressed relays 1 and 2 while never addressing 0 or 3 at all — the
    /// board looked like it had two working relays and two dead ones.
    pub fn relay_state(&mut self, index: u8) -> io::Result<bool> {
        let sel = index;
        let f = self.request(mcu::OP_RELAY_GET, &[sel], Duration::from_millis(400))?;
        let p = &f.payload;
        if p.len() < 2 {
            return Err(io::Error::other("short RELAY_STATE"));
        }
        // Trust the echoed selector: a mismatch means the reply belongs to a
        // different question and reporting it as this relay's state would be a
        // lie about something that may be switching a real load.
        if p[0] != sel {
            return Err(io::Error::other(format!(
                "RELAY_STATE echoed selector 0x{:02x}, asked 0x{:02x}",
                p[0], sel
            )));
        }
        Ok(p[1] != 0)
    }

    /// Every relay, queried one at a time because that is all the protocol
    /// offers.
    /// Drive a relay to a STATE rather than flipping it.
    ///
    /// The MCU has no set opcode — only TOGGLE and GET — so this reads first
    /// and toggles only on a mismatch. That difference matters well beyond
    /// tidiness: a retained MQTT message replayed when a broker reconnects, or
    /// an automation that fires twice, must not leave the relay inverted.
    /// Toggle is not idempotent; this is.
    pub fn relay_set(&mut self, index: u8, on: bool) -> io::Result<bool> {
        if self.relay_state(index)? == on {
            return Ok(on);
        }
        let now = self.relay_toggle(index)?;
        if now != on {
            // The relay did not land where it was asked to. Report it instead
            // of returning the requested value as though it had worked.
            return Err(io::Error::other(format!(
                "relay {index} did not change: asked for {on}, still {now}"
            )));
        }
        Ok(now)
    }

    pub fn relays(&mut self, count: u8) -> io::Result<Vec<bool>> {
        (0..count).map(|i| self.relay_state(i)).collect()
    }

    /// Toggle one relay and return its resulting state. The reply has the same
    /// `[selector, state]` shape as a GET.
    pub fn relay_toggle(&mut self, index: u8) -> io::Result<bool> {
        let sel = index;
        let f = self.request(mcu::OP_RELAY_TOGGLE, &[sel], Duration::from_millis(600))?;
        let p = &f.payload;
        if p.len() < 2 {
            return Err(io::Error::other("short RELAY_STATE after toggle"));
        }
        Ok(p[1] != 0)
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
