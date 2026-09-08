//! Host UART bridge.
//!
//! iod owns every serial port on the controller, so the browser terminal does
//! not open /dev/ttyS* itself — it opens a WebSocket here and iod does the
//! reading. That is the whole point of one owner: two processes with the same
//! tty open both get half the bytes, and which half is a race.
//!
//! Bytes are moved verbatim in both directions. No line discipline, no echo, no
//! translation — an xterm on the other end IS the terminal, and anything this
//! layer "helpfully" rewrites is something the user cannot type.
use crate::mcu::open_serial;
use std::io;
use std::os::unix::io::{FromRawFd, OwnedFd};
use tokio::io::unix::AsyncFd;

pub struct Port {
    fd: AsyncFd<OwnedFd>,
}

impl Port {
    pub fn open(dev: &str, baud: u32) -> io::Result<Port> {
        // open_serial already sets raw 8N1 and O_NONBLOCK, which is what AsyncFd
        // requires — a blocking fd would stall the whole current-thread runtime.
        let raw = open_serial(dev, baud)?;
        let owned = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Port { fd: AsyncFd::new(owned)? })
    }

    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut g = self.fd.readable().await?;
            let n = unsafe {
                libc::read(
                    std::os::unix::io::AsRawFd::as_raw_fd(g.get_inner()),
                    buf.as_mut_ptr() as *mut _,
                    buf.len(),
                )
            };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                // Spurious readiness. clear_ready makes the next await actually
                // wait instead of spinning the runtime at 100%.
                g.clear_ready();
                continue;
            }
            return Err(e);
        }
    }

    pub async fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let mut g = self.fd.writable().await?;
            let n = unsafe {
                libc::write(
                    std::os::unix::io::AsRawFd::as_raw_fd(g.get_inner()),
                    buf.as_ptr() as *const _,
                    buf.len(),
                )
            };
            if n > 0 {
                buf = &buf[n as usize..];
                continue;
            }
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                g.clear_ready();
                continue;
            }
            return Err(e);
        }
        Ok(())
    }
}
