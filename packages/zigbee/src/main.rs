//! ohc-zigbee — bring the EM35x Zigbee NCP up and hold it up.
//!
//! A first-class openHC package (built and staged into the rootfs like iod/webd),
//! gated onto only the boards whose `ohc.features` lists `zigbee`. It replaces the
//! former node script of the same name: the bring-up is a few serial writes, so a
//! ~1 MB static Rust binary beats dragging the Node runtime in just for this.
//! (Node is still pulled by the `zigbee` feature, but for zigbee2mqtt — the
//! MQTT bridge — not for this.)
//!
//! WHY it is needed. The EM35x resets into its serial BOOTLOADER whenever its
//! reset line is asserted, and the bootloader ignores EZSP/ASH. On the CA-1 that
//! reset is the port's DTR, so merely opening the tty (what every EZSP host does)
//! lands in the bootloader and the coordinator never forms. The fix, in order:
//!
//!   1. open the port   (DTR asserted -> on DTR-reset boards the chip drops to
//!                        its bootloader, which echoes)
//!   2. send "2"        (the bootloader's "run application" menu key)
//!   3. ASH RST -> RSTACK confirms the NCP is now running
//!   4. HOLD the port open for the life of the service so DTR stays asserted —
//!      the next opener (zigbee2mqtt) does NOT re-trigger the bootloader.
//!
//! On boards whose reset is a GPIO held released at boot (the EA's gpio29), step 1
//! does not reset anything — the app is already running — and the same sequence
//! simply confirms it. Verified on an EA1: a bare ASH RST returns RSTACK (0xc1).
//!
//! Config (board.env): OHC_ZIGBEE_TTY (required; absent => nothing to do),
//! OHC_ZIGBEE_BAUD (default 115200).

use std::io;
use std::os::unix::io::RawFd;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// ASH software reset frame: a running NCP answers with a RSTACK (carries 0xc1).
const ASH_RST: [u8; 5] = [0x1a, 0xc0, 0x38, 0xbc, 0x7e];

fn write_all(fd: RawFd, buf: &[u8]) {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::write(fd, buf[off..].as_ptr() as *const libc::c_void, buf.len() - off)
        };
        if n > 0 {
            off += n as usize;
        } else {
            break;
        }
    }
}

/// Collect whatever arrives over `ms` (the fd is O_NONBLOCK).
fn read_for(fd: RawFd, ms: u64) -> Vec<u8> {
    let end = Instant::now() + Duration::from_millis(ms);
    let mut out = Vec::new();
    let mut b = [0u8; 256];
    while Instant::now() < end {
        let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, b.len()) };
        if n > 0 {
            out.extend_from_slice(&b[..n as usize]);
        } else {
            sleep(Duration::from_millis(5));
        }
    }
    out
}

fn bring_up(fd: RawFd) -> bool {
    write_all(fd, b"\r\n");
    let _ = read_for(fd, 1200); // wake the bootloader (it echoes)
    write_all(fd, b"2");
    let _ = read_for(fd, 2000); // "2" = run application
    write_all(fd, &ASH_RST);
    read_for(fd, 1500).contains(&0xc1) // RSTACK
}

/// Open a tty raw at `baud`, 8N1, no flow control. Mirrors iod's open_serial;
/// libc termios rather than a serialport crate, which drags a dependency tree
/// (and libudev) in for one tcsetattr and fights musl static cross-builds.
fn open_serial(dev: &str, baud: u32) -> io::Result<RawFd> {
    use std::ffi::CString;
    let path = CString::new(dev).map_err(|_| io::Error::other("bad device path"))?;
    let fd = unsafe {
        libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let speed = match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
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
        // Assert DTR: the CA-1's EM35x reset line. Harmless where reset is a GPIO.
        let bits: libc::c_int = libc::TIOCM_DTR;
        libc::ioctl(fd, libc::TIOCMBIS, &bits);
    }
    Ok(fd)
}

fn main() {
    let dev = match std::env::var("OHC_ZIGBEE_TTY") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            println!("ohc-zigbee: OHC_ZIGBEE_TTY unset — no radio on this board, nothing to do");
            return;
        }
    };
    let baud: u32 = std::env::var("OHC_ZIGBEE_BAUD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(115200);

    let fd = match open_serial(&dev, baud) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("ohc-zigbee: cannot open {dev}: {e}");
            std::process::exit(1);
        }
    };

    let mut up = false;
    for _ in 0..4 {
        if bring_up(fd) {
            up = true;
            break;
        }
    }
    if up {
        println!("ohc-zigbee: NCP up on {dev} (RSTACK)");
    } else {
        println!("ohc-zigbee: NCP did not answer on {dev} after 4 tries — holding the port anyway");
    }

    // Hold the fd (and DTR) open forever. Deliberately no reads past bring-up: the
    // radio owner (zigbee2mqtt) reads the EZSP stream, and a tty's input is a
    // single queue — a read here would steal its bytes.
    loop {
        sleep(Duration::from_secs(3600));
    }
}
