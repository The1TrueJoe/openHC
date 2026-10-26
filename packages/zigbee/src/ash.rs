//! ASH + EZSP, just enough to identify an EM35x NCP and send it to its
//! bootloader: the version command and launchStandaloneBootloader.
//!
//! ASH (Silicon Labs UG101) is the serial framing under EZSP: frames end in a
//! Flag byte, reserved bytes are escaped, DATA payloads are XORed with a fixed
//! pseudo-random sequence, every frame carries a CRC-CCITT, and DATA frames carry
//! 3-bit sequence and acknowledge numbers. EZSP (UG100) rides inside DATA frames:
//! `[seq, frameControl, frameId, params…]` up to protocol 7, and from protocol 8
//! the extended `[seq, fcLo, fcHi, idLo, idHi, params…]` — except the version
//! command, which is always sent in the legacy form so any NCP can answer it.
//!
//! Not a radio stack: no callbacks, no retransmission beyond a resend on silence.
//! Whatever owns the radio afterwards (zigpy, zigbee2mqtt) resets ASH itself.

use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

pub const FLAG: u8 = 0x7e;
const ESC: u8 = 0x7d;
const XON: u8 = 0x11;
const XOFF: u8 = 0x13;
const SUBST: u8 = 0x18;
const CANCEL: u8 = 0x1a;
const RESERVED: [u8; 6] = [FLAG, ESC, XON, XOFF, SUBST, CANCEL];

/// RST, preceded by a Cancel so any partial frame in the NCP is discarded.
pub const RST: [u8; 5] = [CANCEL, 0xc0, 0x38, 0xbc, FLAG];
const RSTACK: u8 = 0xc1;

/// EZSP frame IDs used here.
pub const EZSP_VERSION: u16 = 0x0000;
pub const EZSP_LAUNCH_STANDALONE_BOOTLOADER: u16 = 0x008f;
/// launchStandaloneBootloader mode: STANDALONE_BOOTLOADER_NORMAL_MODE.
pub const BOOTLOADER_NORMAL_MODE: u8 = 1;

/// CRC-CCITT (poly 0x1021), as ASH (init 0xffff) and XMODEM (init 0) use it.
pub fn crc16(data: &[u8], init: u16) -> u16 {
    let mut c = init;
    for &b in data {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// XOR `data` with ASH's pseudo-random sequence (seed 0x42). Its own inverse.
pub fn randomize(data: &[u8]) -> Vec<u8> {
    let mut r: u8 = 0x42;
    data.iter()
        .map(|&b| {
            let o = b ^ r;
            r = if r & 1 != 0 { (r >> 1) ^ 0xb8 } else { r >> 1 };
            o
        })
        .collect()
}

pub fn stuff(data: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(data.len() + 4);
    for &b in data {
        if RESERVED.contains(&b) {
            o.push(ESC);
            o.push(b ^ 0x20);
        } else {
            o.push(b);
        }
    }
    o
}

pub fn unstuff(data: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(data.len());
    let mut esc = false;
    for &b in data {
        if b == ESC {
            esc = true;
            continue;
        }
        o.push(if esc { b ^ 0x20 } else { b });
        esc = false;
    }
    o
}

/// One frame on the wire: stuffed `control + body + CRC`, then a Flag.
pub fn frame(control: u8, body: &[u8]) -> Vec<u8> {
    let mut f = vec![control];
    f.extend_from_slice(body);
    let c = crc16(&f, 0xffff);
    f.push((c >> 8) as u8);
    f.push(c as u8);
    let mut o = stuff(&f);
    o.push(FLAG);
    o
}

/// Splits a byte stream into unstuffed frames, CRC-checked. Cancel and
/// Substitute discard the frame in progress; XON/XOFF (software flow control
/// from a "uart-sw" NCP) are not frame bytes and are dropped.
#[derive(Default)]
pub struct Deframer {
    cur: Vec<u8>,
    bad: bool,
}

impl Deframer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                FLAG => {
                    let raw = std::mem::take(&mut self.cur);
                    let bad = std::mem::replace(&mut self.bad, false);
                    if bad || raw.is_empty() {
                        continue;
                    }
                    let f = unstuff(&raw);
                    if f.len() >= 3 {
                        let (body, c) = f.split_at(f.len() - 2);
                        if crc16(body, 0xffff) == u16::from_be_bytes([c[0], c[1]]) {
                            out.push(body.to_vec());
                        }
                    }
                }
                CANCEL => {
                    self.cur.clear();
                    self.bad = false;
                }
                SUBST => self.bad = true,
                XON | XOFF => {}
                _ => self.cur.push(b),
            }
        }
        out
    }
}

/// Version as the NCP reports it: EZSP protocol, stack type, and the EmberZNet
/// stack version, nibble-coded (0x6780 = 6.7.8.0) so it compares numerically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NcpVersion {
    pub protocol: u8,
    pub stack_type: u8,
    pub stack: u16,
}

pub fn stack_str(v: u16) -> String {
    format!("{}.{}.{}.{}", v >> 12, (v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf)
}

/// "6.7.8" or "6.7.8.0" -> 0x6780.
pub fn parse_stack(s: &str) -> Option<u16> {
    let parts: Vec<u16> = s.trim().split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|&p| p > 15) {
        return None;
    }
    Some(parts.iter().chain(std::iter::repeat(&0)).take(4).fold(0, |a, &p| (a << 4) | p))
}

/// An ASH session on an already-open, raw, non-blocking tty.
pub struct Ash {
    fd: RawFd,
    tx: u8,
    rx: u8,
    seq: u8,
    /// EZSP protocol in use, once known: decides legacy vs extended headers.
    protocol: u8,
    de: Deframer,
}

impl Ash {
    pub fn new(fd: RawFd) -> Self {
        Ash { fd, tx: 0, rx: 0, seq: 0, protocol: 4, de: Deframer::default() }
    }

    fn write(&self, b: &[u8]) {
        crate::write_all(self.fd, b);
    }

    /// Frames that arrive within `ms`, stopping early once `done` is satisfied.
    fn read_frames(&mut self, ms: u64, mut done: impl FnMut(&[u8]) -> bool) -> Vec<Vec<u8>> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        let mut b = [0u8; 256];
        while Instant::now() < end {
            let n = unsafe { libc::read(self.fd, b.as_mut_ptr() as *mut libc::c_void, b.len()) };
            if n > 0 {
                for f in self.de.push(&b[..n as usize]) {
                    let stop = done(&f);
                    out.push(f);
                    if stop {
                        return out;
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        out
    }

    /// ASH reset. True when the NCP answers RSTACK; sequence numbers restart.
    pub fn reset(&mut self) -> bool {
        self.tx = 0;
        self.rx = 0;
        self.seq = 0;
        self.de = Deframer::default();
        self.write(&RST);
        self.read_frames(2000, |f| f[0] == RSTACK).iter().any(|f| f[0] == RSTACK)
    }

    fn data_frame(&mut self, payload: &[u8]) -> Vec<u8> {
        let control = (self.tx << 4) | self.rx;
        self.tx = (self.tx + 1) & 7;
        frame(control, &randomize(payload))
    }

    fn ack(&self) -> Vec<u8> {
        frame(0x80 | self.rx, &[])
    }

    /// Send an EZSP command and return the matching response's parameters.
    pub fn ezsp(&mut self, id: u16, params: &[u8]) -> Option<Vec<u8>> {
        let legacy = id == EZSP_VERSION || self.protocol < 8;
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        let mut p = if legacy {
            vec![seq, 0x00, id as u8]
        } else {
            vec![seq, 0x00, 0x01, id as u8, (id >> 8) as u8]
        };
        p.extend_from_slice(params);
        for _attempt in 0..3 {
            let f = self.data_frame(&p);
            self.write(&f);
            let mut reply = None;
            let frames = self.read_frames(1500, |fr| {
                if fr[0] & 0x80 != 0 {
                    return false; // ACK/NAK/RSTACK/ERROR, not data
                }
                let d = randomize(&fr[1..]);
                let hit = if legacy {
                    d.len() >= 3 && d[0] == seq && d[1] & 0x80 != 0 && d[2] == id as u8
                } else {
                    d.len() >= 5 && d[0] == seq && d[1] & 0x80 != 0
                        && u16::from_le_bytes([d[3], d[4]]) == id
                };
                if hit {
                    reply = Some(d[if legacy { 3 } else { 5 }..].to_vec());
                }
                hit
            });
            for fr in &frames {
                if fr[0] & 0x80 == 0 {
                    self.rx = ((fr[0] >> 4) + 1) & 7;
                    let a = self.ack();
                    self.write(&a);
                }
            }
            if reply.is_some() {
                return reply;
            }
            // Nothing for us: the NCP missed or rejected the frame. Resend with
            // the same EZSP sequence, as a fresh ASH frame.
        }
        None
    }

    /// EZSP version handshake. An NCP on a newer protocol answers with its own
    /// version; ask again with that, as every EZSP host does, so later commands
    /// use the header format it expects.
    pub fn version(&mut self) -> Option<NcpVersion> {
        let mut want = 4u8;
        for _ in 0..2 {
            let r = self.ezsp(EZSP_VERSION, &[want])?;
            if r.len() < 4 {
                return None;
            }
            let v = NcpVersion { protocol: r[0], stack_type: r[1], stack: u16::from_le_bytes([r[2], r[3]]) };
            self.protocol = v.protocol;
            if v.protocol == want {
                return Some(v);
            }
            want = v.protocol;
        }
        None
    }

    /// Ask the running NCP to jump to its serial bootloader. Some(0) = EMBER_SUCCESS.
    pub fn launch_bootloader(&mut self) -> Option<u8> {
        self.ezsp(EZSP_LAUNCH_STANDALONE_BOOTLOADER, &[BOOTLOADER_NORMAL_MODE])
            .and_then(|r| r.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_vectors() {
        // CRC-16/CCITT-FALSE and CRC-16/XMODEM of "123456789".
        assert_eq!(crc16(b"123456789", 0xffff), 0x29b1);
        assert_eq!(crc16(b"123456789", 0), 0x31c3);
        // The RST frame's CRC is the published 0x38bc.
        assert_eq!(crc16(&[0xc0], 0xffff), 0x38bc);
        assert_eq!(frame(0xc0, &[]), vec![0xc0, 0x38, 0xbc, FLAG]);
    }

    #[test]
    fn randomize_sequence() {
        // UG101's sequence starts 42 21 A8 54 2A.
        assert_eq!(randomize(&[0; 5]), vec![0x42, 0x21, 0xa8, 0x54, 0x2a]);
        let d = [1, 2, 3, 0x7e, 0xff];
        assert_eq!(randomize(&randomize(&d)), d);
    }

    #[test]
    fn stuffing_round_trip() {
        let d: Vec<u8> = (0..=255).collect();
        let s = stuff(&d);
        assert!(!s.contains(&FLAG));
        assert_eq!(unstuff(&s), d);
    }

    #[test]
    fn deframer_checks_crc_and_ignores_flow_control() {
        let mut d = Deframer::default();
        let mut wire = vec![XON, 0x00]; // noise then a Cancel resets it
        wire.push(CANCEL);
        wire.extend(frame(0xc1, &[0x02, 0x0b]));
        wire.push(XOFF);
        let mut broken = frame(0x25, &[1, 2, 3]);
        broken[1] ^= 0x01; // corrupt: CRC must reject it
        wire.extend(broken);
        let f = d.push(&wire);
        assert_eq!(f, vec![vec![0xc1, 0x02, 0x0b]]);
    }

    #[test]
    fn stack_versions() {
        assert_eq!(parse_stack("6.7.8"), Some(0x6780));
        assert_eq!(parse_stack("6.7.8.0"), Some(0x6780));
        assert_eq!(parse_stack("4.7.2"), Some(0x4720));
        assert_eq!(parse_stack("16.0"), None);
        assert_eq!(stack_str(0x4720), "4.7.2.0");
        assert!(parse_stack("4.7.2").unwrap() < parse_stack("6.7.8").unwrap());
    }
}
