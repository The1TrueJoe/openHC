//! A minimal read-only TFTP server (RFC 1350) for the netboot bring-up.
//!
//! CEFDK's `tftp get` is the only file transfer the manufacturing shell has, so
//! this exists purely to feed it the kernel and initramfs. It implements exactly
//! what that client uses — RRQ in, DATA/ACK out, octet mode, 512-byte blocks —
//! and nothing else. WRQ (writes) are refused: we never accept a file from the
//! box.
//!
//! Standard TID behaviour: the reply DATA comes from a FRESH ephemeral port, not
//! from :69, and the client latches onto that port for the rest of the transfer.
//! U-Boot-lineage clients (CEFDK among them) expect this; serving DATA back from
//! :69 makes some of them reject the transfer.
//!
//! Binding :69 needs root — that is expected; the CLI is run under sudo. We do
//! not try to drop privileges.

use std::io::{self};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// TFTP opcodes we deal in.
const OP_RRQ: u16 = 1;
const OP_WRQ: u16 = 2;
const OP_DATA: u16 = 3;
const OP_ACK: u16 = 4;
const OP_ERROR: u16 = 5;

/// The one and only block size we serve. 512 is the classic default every TFTP
/// client understands without the blksize option, and it is what CEFDK uses.
const BLOCK: usize = 512;

/// A running server. Dropping it stops the thread; [`stop`](TftpServer::stop)
/// does so explicitly and joins.
pub struct TftpServer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub bound: SocketAddr,
}

impl TftpServer {
    /// Bind `0.0.0.0:69` and serve `dir` read-only on a background thread.
    ///
    /// `announce` is called (from the server thread) with a short human line
    /// each time a file is requested and when it finishes, so the front end can
    /// show transfer progress next to the serial log.
    pub fn start(
        dir: impl Into<PathBuf>,
        announce: impl Fn(String) + Send + 'static,
    ) -> io::Result<TftpServer> {
        let dir = dir.into();
        // A short read timeout is what lets the loop notice `stop` promptly
        // instead of blocking forever in recv_from.
        let sock = UdpSocket::bind(("0.0.0.0", 69))?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))?;
        let bound = sock.local_addr()?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || serve_loop(sock, &dir, &stop_thread, &announce));
        Ok(TftpServer { stop, handle: Some(handle), bound })
    }

    /// Signal the thread and wait for it to wind down.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for TftpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve_loop(
    sock: UdpSocket,
    dir: &Path,
    stop: &AtomicBool,
    announce: &(dyn Fn(String) + Send),
) {
    let mut buf = [0u8; BLOCK + 4];
    while !stop.load(Ordering::SeqCst) {
        let (n, peer) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                continue
            }
            Err(_) => continue,
        };
        match parse_request(&buf[..n]) {
            Some(Request::Read { filename, .. }) => {
                // Refuse to escape the served directory: reject any path
                // separator or parent ref. Bring-up serves flat filenames only.
                if is_unsafe_name(&filename) {
                    let _ = sock.send_to(&error_packet(2, "access violation"), peer);
                    announce(format!("tftp: refused unsafe name {filename:?}"));
                    continue;
                }
                announce(format!("tftp: {filename} -> {}", peer.ip()));
                match std::fs::read(dir.join(&filename)) {
                    Ok(data) => match send_file(&peer, &data, stop) {
                        Ok(()) => announce(format!("tftp: sent {filename} ({} B)", data.len())),
                        Err(e) => announce(format!("tftp: {filename} transfer failed: {e}")),
                    },
                    Err(_) => {
                        let _ = sock.send_to(&error_packet(1, "file not found"), peer);
                        announce(format!("tftp: {filename} not found in {}", dir.display()));
                    }
                }
            }
            Some(Request::Write) => {
                // Read-only by design.
                let _ = sock.send_to(&error_packet(2, "read-only server"), peer);
            }
            None => {} // not a request we handle; ignore
        }
    }
}

/// Send one file over a fresh data socket, block by block, waiting for each
/// ACK. Returns on the final short block being ACKed. The stop flag is checked
/// between blocks so a shutdown mid-transfer is not blocked on a dead client.
fn send_file(peer: &SocketAddr, data: &[u8], stop: &AtomicBool) -> io::Result<()> {
    // Fresh ephemeral port = our TID; the client replies here from now on.
    let data_sock = UdpSocket::bind(("0.0.0.0", 0))?;
    data_sock.set_read_timeout(Some(Duration::from_millis(800)))?;

    // A file that is an exact multiple of 512 needs a trailing EMPTY block so
    // the client knows the transfer ended; `chunks` would omit it, so drive the
    // block index explicitly and stop AFTER sending a short (< 512) block.
    let mut block: u16 = 1;
    let mut off = 0usize;
    let mut ack = [0u8; 4];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "server stopping"));
        }
        let end = (off + BLOCK).min(data.len());
        let chunk = &data[off..end];
        let pkt = data_packet(block, chunk);

        // Up to a handful of resends per block covers a dropped datagram on the
        // bring-up link without turning a genuinely gone client into a hang.
        let mut acked = false;
        for _ in 0..5 {
            data_sock.send_to(&pkt, peer)?;
            match data_sock.recv_from(&mut ack) {
                Ok((n, from)) if from.ip() == peer.ip() => {
                    if parse_ack(&ack[..n]) == Some(block) {
                        acked = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e),
            }
        }
        if !acked {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no ACK from client"));
        }

        // A block shorter than 512 (including a zero-length final block) is the
        // last one.
        if chunk.len() < BLOCK {
            return Ok(());
        }
        off = end;
        block = block.wrapping_add(1);
    }
}

/// What kind of request a datagram is, and (for a read) the filename.
enum Request {
    Read { filename: String, #[allow(dead_code)] octet: bool },
    Write,
}

/// Parse an RRQ/WRQ. Returns None for anything else. Pure — unit-tested.
///
/// Layout: opcode (u16 BE), then NUL-terminated filename, then NUL-terminated
/// mode ("octet"/"netascii", case-insensitive per the RFC).
fn parse_request(pkt: &[u8]) -> Option<Request> {
    if pkt.len() < 2 {
        return None;
    }
    let opcode = u16::from_be_bytes([pkt[0], pkt[1]]);
    match opcode {
        OP_WRQ => Some(Request::Write),
        OP_RRQ => {
            let mut parts = pkt[2..].split(|&b| b == 0);
            let filename = parts.next()?;
            if filename.is_empty() {
                return None;
            }
            let mode = parts.next().unwrap_or(b"");
            let filename = String::from_utf8_lossy(filename).into_owned();
            let octet = mode.eq_ignore_ascii_case(b"octet");
            Some(Request::Read { filename, octet })
        }
        _ => None,
    }
}

/// Reject filenames that would climb out of the served directory. Bring-up only
/// ever asks for flat names like `bzImage`.
fn is_unsafe_name(name: &str) -> bool {
    name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || Path::new(name).is_absolute()
}

/// A DATA packet: opcode 3, block number (BE), then up to 512 bytes. Pure.
fn data_packet(block: u16, chunk: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(4 + chunk.len());
    p.extend_from_slice(&OP_DATA.to_be_bytes());
    p.extend_from_slice(&block.to_be_bytes());
    p.extend_from_slice(chunk);
    p
}

/// The block number an ACK acknowledges, or None if the packet is not an ACK.
/// Pure.
fn parse_ack(pkt: &[u8]) -> Option<u16> {
    if pkt.len() < 4 || u16::from_be_bytes([pkt[0], pkt[1]]) != OP_ACK {
        return None;
    }
    Some(u16::from_be_bytes([pkt[2], pkt[3]]))
}

/// An ERROR packet: opcode 5, error code (BE), NUL-terminated message.
fn error_packet(code: u16, msg: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(5 + msg.len());
    p.extend_from_slice(&OP_ERROR.to_be_bytes());
    p.extend_from_slice(&code.to_be_bytes());
    p.extend_from_slice(msg.as_bytes());
    p.push(0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use ohc_flash_core::cefdk;

    #[test]
    fn block_size_matches_the_sector_the_rest_of_the_tool_uses() {
        // Not load-bearing, but a sanity tie: a 512-byte block IS the eMMC
        // sector, so anyone reading both files sees the same magic number mean
        // the same thing.
        assert_eq!(BLOCK as u64, cefdk::SECTOR);
    }

    #[test]
    fn rrq_octet_is_parsed_into_a_read() {
        let mut pkt = vec![0, OP_RRQ as u8];
        pkt.extend_from_slice(b"bzImage\0octet\0");
        match parse_request(&pkt) {
            Some(Request::Read { filename, octet }) => {
                assert_eq!(filename, "bzImage");
                assert!(octet, "CEFDK asks in octet mode");
            }
            _ => panic!("expected a read request"),
        }
    }

    #[test]
    fn a_wrq_is_recognised_so_it_can_be_refused() {
        let pkt = [0, OP_WRQ as u8, b'x', 0, b'o', b'c', b't', b'e', b't', 0];
        assert!(matches!(parse_request(&pkt), Some(Request::Write)));
    }

    #[test]
    fn junk_is_not_a_request() {
        assert!(parse_request(&[]).is_none());
        assert!(parse_request(&[0]).is_none());
        assert!(parse_request(&[0, 99, b'x', 0]).is_none(), "unknown opcode");
    }

    #[test]
    fn data_frame_has_opcode_block_then_payload() {
        let p = data_packet(1, b"hi");
        assert_eq!(&p[..2], &[0, OP_DATA as u8]);
        assert_eq!(&p[2..4], &[0, 1], "block 1, big-endian");
        assert_eq!(&p[4..], b"hi");
    }

    #[test]
    fn ack_round_trips_the_block_number() {
        // Block numbers wrap past 65535 on a big file; the u16 is exactly what
        // the ACK carries, so match on it as-is.
        let ack = [0, OP_ACK as u8, 0x12, 0x34];
        assert_eq!(parse_ack(&ack), Some(0x1234));
        assert_eq!(parse_ack(&[0, OP_DATA as u8, 0, 1]), None, "a DATA packet is not an ACK");
        assert_eq!(parse_ack(&[0, OP_ACK as u8]), None, "too short to hold a block number");
    }

    #[test]
    fn unsafe_names_are_rejected() {
        assert!(is_unsafe_name("../etc/passwd"));
        assert!(is_unsafe_name("/etc/passwd"));
        assert!(is_unsafe_name("sub/dir"));
        assert!(!is_unsafe_name("bzImage"));
        assert!(!is_unsafe_name("rootfs.cpio.gz"));
    }
}
