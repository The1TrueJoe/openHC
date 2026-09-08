//! The IO microcontroller link: DLE/STX framing over a UART.
//!
//! Every frame is
//!
//!     10 02 | opcode | seq | flags | len16 BE | payload... | checksum
//!
//! and the checksum is the negated 8-bit sum of everything after STX. Any 0x10
//! inside the body is escaped by doubling it. The reply opcode is the request
//! opcode + 1, with ONE exception noted at `AUTO_BAUD`.
//!
//! All of this is confirmed against a live HC-800 (LM3S1162 @115200) rather
//! than inferred:
//!
//!     --> 10 02 34 01 00 00 00 cb              FIRMWARE_VERSION_GET
//!     <-- 10 02 35 01 02 00 08 "03.26.15" 33
//!     --> 10 02 74 12 00 00 00 7a              CONTACT_GET
//!     <-- 10 02 75 12 02 00 04 00 00 00 00 73
//!
//! The EA family speaks the same protocol on a TM4C1231D5 at 460800.
// Some opcodes below are not called yet (IR learning). They stay because this
// module is the protocol's documentation as much as its implementation — a
// reader checking a capture against the wire should find every opcode here.
#![allow(dead_code)]

use std::io;
use std::os::unix::io::RawFd;

pub const DLE: u8 = 0x10;
pub const STX: u8 = 0x02;
pub const FLAG_RESPONSE: u8 = 0x02;

// Opcodes. Replies are request+1 throughout except AUTO_BAUD.

pub const OP_PRODUCT_NAME: u8 = 0x24;
pub const OP_FIRMWARE_VERSION: u8 = 0x34;
pub const OP_RELAY_GET: u8 = 0x54;
pub const OP_RELAY_TOGGLE: u8 = 0x56;
pub const OP_IROUT_SEND: u8 = 0x66;
pub const OP_CONTACT_GET: u8 = 0x74;
pub const OP_IRIN_SET_CAPTURE: u8 = 0x77;
pub const OP_IRIN_CAPTURED: u8 = 0x97;
/// The exception: 0xD2 is answered by 0xD7, not 0xD3. Its payload is a 32-bit
/// BIG-ENDIAN BAUD RATE — an HC-800 answers 0x0001c207 = 115207, which is 0.006%
/// off nominal because it is measured, and an EA answers 0x00070800 = exactly
/// 460800. Older notes read the leading `00 07` as a header; it is not.
pub const OP_AUTO_BAUD: u8 = 0xD2;
pub const OP_AUTO_BAUD_RESP: u8 = 0xD7;

#[derive(Debug, Clone)]
pub struct Frame {
    pub opcode: u8,
    pub seq: u8,
    pub flags: u8,
    pub payload: Vec<u8>,
}

pub fn checksum(body: &[u8]) -> u8 {
    (!body.iter().fold(0u8, |a, b| a.wrapping_add(*b))).wrapping_add(1)
}

/// Build a wire frame, applying DLE stuffing to the body.
pub fn encode(opcode: u8, seq: u8, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + payload.len());
    body.push(opcode);
    body.push(seq);
    body.push(flags);
    body.push((payload.len() >> 8) as u8);
    body.push(payload.len() as u8);
    body.extend_from_slice(payload);
    let ck = checksum(&body);

    let mut out = vec![DLE, STX];
    for &b in body.iter().chain(std::iter::once(&ck)) {
        out.push(b);
        if b == DLE {
            out.push(DLE); // stuff
        }
    }
    out
}

/// Incremental receiver. Fed bytes as they arrive; yields whole, checksum-valid
/// frames. Malformed input resynchronises rather than wedging — a UART sees
/// line noise and half-frames on every reset, and a decoder that can only be
/// recovered by restarting the daemon is not usable on real hardware.
#[derive(Default)]
pub struct Decoder {
    state: State,
    body: Vec<u8>,
    pub resyncs: u64,
    pub bad_checksums: u64,
}

#[derive(Default, PartialEq)]
enum State {
    #[default]
    Idle, // hunting for DLE
    Stx,  // saw DLE, want STX
    Body,
    BodyDle, // saw DLE in the body; a second means a literal 0x10
}

impl Decoder {
    pub fn feed(&mut self, data: &[u8], mut on_frame: impl FnMut(Frame)) {
        for &b in data {
            match self.state {
                State::Idle => {
                    if b == DLE {
                        self.state = State::Stx;
                    }
                }
                State::Stx => {
                    if b == STX {
                        self.body.clear();
                        self.state = State::Body;
                    } else if b == DLE {
                        // stay: back-to-back DLEs while hunting
                    } else {
                        self.resyncs += 1;
                        self.state = State::Idle;
                    }
                }
                State::Body => {
                    if b == DLE {
                        self.state = State::BodyDle;
                    } else {
                        self.body.push(b);
                        self.try_complete(&mut on_frame);
                    }
                }
                State::BodyDle => {
                    if b == DLE {
                        self.body.push(DLE); // un-stuff
                        self.state = State::Body;
                        self.try_complete(&mut on_frame);
                    } else if b == STX {
                        // A new frame started mid-frame: abandon and take it.
                        self.resyncs += 1;
                        self.body.clear();
                        self.state = State::Body;
                    } else {
                        self.resyncs += 1;
                        self.state = State::Idle;
                    }
                }
            }
        }
    }

    fn try_complete(&mut self, on_frame: &mut impl FnMut(Frame)) {
        if self.body.len() < 5 {
            return;
        }
        let len = ((self.body[3] as usize) << 8) | self.body[4] as usize;
        let want = 5 + len + 1; // header + payload + checksum
        if self.body.len() < want {
            return;
        }
        let ck = self.body[want - 1];
        if checksum(&self.body[..want - 1]) == ck {
            on_frame(Frame {
                opcode: self.body[0],
                seq: self.body[1],
                flags: self.body[2],
                payload: self.body[5..want - 1].to_vec(),
            });
        } else {
            self.bad_checksums += 1;
        }
        self.body.clear();
        self.state = State::Idle;
    }
}

/// Open a UART in raw 8N1 at `baud` and return the fd.
///
/// Written against libc termios rather than a serialport crate: the crate pulls
/// a dependency tree for what is one tcsetattr, and this has to cross-compile
/// to three targets.
pub fn open_serial(dev: &str, baud: u32) -> io::Result<RawFd> {
    use std::ffi::CString;
    let path = CString::new(dev).map_err(|_| io::Error::other("bad device path"))?;
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // The high rates are Linux-only constants. This daemon only ever runs on
    // Linux, but keeping the host build compiling is worth a cfg: without it
    // `cargo check` on a Mac fails on B460800 and there is no local way to
    // syntax-check the file at all (Homebrew's cargo has no cross std).
    //
    // 460800 is not academic here — it is the EA family's IO-MCU rate, where
    // the HC-800 runs 115200.
    #[cfg(target_os = "linux")]
    let speed = match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        460800 => libc::B460800,
        921600 => libc::B921600,
        _ => libc::B115200,
    };
    #[cfg(not(target_os = "linux"))]
    let speed = match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        _ => libc::B115200,
    };
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            let e = io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        libc::cfmakeraw(&mut t);
        libc::cfsetispeed(&mut t, speed);
        libc::cfsetospeed(&mut t, speed);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        t.c_cflag &= !libc::CRTSCTS;
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
            let e = io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        libc::tcflush(fd, libc::TCIOFLUSH);
    }
    Ok(fd)
}
