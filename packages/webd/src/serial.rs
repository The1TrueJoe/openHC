//! Raw serial via libc termios — no serialport crate (keeps cross-compilation
//! dependency-free). Opens /dev/ttymxcN, sets raw 8N1 at the requested baud, and
//! offers blocking read/write plus a non-blocking poll for the WS bridge.
use std::os::unix::io::{FromRawFd, RawFd};

pub struct Serial {
    file: std::fs::File,
}

fn baud_const(baud: u32) -> libc::speed_t {
    match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        460800 => libc::B460800,
        921600 => libc::B921600,
        _ => libc::B115200,
    }
}

impl Serial {
    pub fn open(dev: &str, baud: u32) -> std::io::Result<Serial> {
        // O_NONBLOCK on open so a dead line can't hang us; cleared after.
        let fd: RawFd = unsafe {
            libc::open(
                std::ffi::CString::new(dev)?.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                libc::close(fd);
                return Err(std::io::Error::last_os_error());
            }
            libc::cfmakeraw(&mut t);
            t.c_cflag |= libc::CS8 | libc::CREAD | libc::CLOCAL;
            t.c_cflag &= !(libc::PARENB | libc::CSTOPB | libc::CRTSCTS);
            let sp = baud_const(baud);
            libc::cfsetispeed(&mut t, sp);
            libc::cfsetospeed(&mut t, sp);
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
                libc::close(fd);
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Serial { file: unsafe { std::fs::File::from_raw_fd(fd) } })
    }

    pub fn write_all(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use std::io::Write;
        let mut off = 0;
        while off < buf.len() {
            match self.file.write(&buf[off..]) {
                Ok(0) => break,
                Ok(n) => off += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(off)
    }

    /// Drain whatever is available over `ms` milliseconds (for the radio rx probe).
    pub fn read_for(&mut self, ms: u64) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut tmp = [0u8; 512];
        while std::time::Instant::now() < end {
            match self.file.read(&mut tmp) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(n) => out.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        out
    }

    /// One non-blocking read for the WS stream loop (returns empty on WouldBlock).
    pub fn try_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Read;
        match self.file.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        }
    }
}
