//! Serial transport for driving the CEFDK console.
//!
//! The EA netboot bring-up types commands at CEFDK's `shell>` and watches its
//! output. That is line-oriented at 115200 8N1, so this wraps the raw port in a
//! tiny type that gives the engine three operations: pull whatever COMPLETE
//! lines have arrived (to feed the pure `MfgWatch` state machine), look at the
//! un-terminated tail (where the newline-less `shell>` prompt sits) and write a
//! command terminated the way the shell expects.
//!
//! Like `ssh.rs`, the backend sits behind a small struct so a different serial
//! library (or a mock, for a bench-free test) could replace `serialport` without
//! the engine noticing.

use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

/// CEFDK's console. 115200 8N1 is what the boot ROM and CEFDK print at, and the
/// only rate the manufacturing shell listens on.
pub const CEFDK_BAUD: u32 = 115_200;

/// Errors opening or talking to the port. A plain enum with hand-written
/// `Display`, matching `SshError` — not worth a proc-macro for three cases.
#[derive(Debug)]
pub enum SerialError {
    Open { dev: String, msg: String },
    Io(String),
}

impl std::fmt::Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerialError::Open { dev, msg } => write!(
                f,
                "cannot open serial port {dev}: {msg} \
                 (is the adapter plugged in, and is this the right /dev node?)"
            ),
            SerialError::Io(m) => write!(f, "serial I/O error: {m}"),
        }
    }
}

impl std::error::Error for SerialError {}

/// An open CEFDK console.
///
/// Reads are chunked off the wire and re-assembled into lines by a
/// [`LineBuffer`], because a UART hands you bytes whenever they arrive — a
/// single console line routinely straddles two reads, and a command echoed back
/// can arrive a character at a time.
pub struct Serial {
    port: Box<dyn serialport::SerialPort>,
    buf: LineBuffer,
    pub dev: String,
    pub baud: u32,
}

impl Serial {
    /// Open `dev` at `baud`, 8N1. The per-read timeout is set fresh on every
    /// `poll` call, so the value passed here is only a placeholder.
    pub fn open(dev: &str, baud: u32) -> Result<Serial, SerialError> {
        let port = serialport::new(dev, baud)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| SerialError::Open { dev: dev.to_string(), msg: e.to_string() })?;
        Ok(Serial { port, buf: LineBuffer::default(), dev: dev.to_string(), baud })
    }

    /// Read whatever bytes arrive within `timeout` into the buffer. Returns
    /// whether ANY byte arrived: `false` is real silence, which is not the same
    /// as "no line completed" — a read can deliver half a line, and on Windows
    /// the COM driver routinely hands over a few bytes at a time.
    pub fn poll(&mut self, timeout: Duration) -> Result<bool, SerialError> {
        self.port
            .set_timeout(timeout)
            .map_err(|e| SerialError::Io(e.to_string()))?;
        let mut chunk = [0u8; 512];
        match self.port.read(&mut chunk) {
            Ok(n) => {
                self.buf.push(&chunk[..n]);
                Ok(n > 0)
            }
            // A timeout is the normal "nothing arrived this window" case, not an
            // error. Some platforms report an empty read the same way.
            Err(e) if e.kind() == ErrorKind::TimedOut => Ok(false),
            Err(e) => Err(SerialError::Io(e.to_string())),
        }
    }

    /// The COMPLETE lines buffered so far (terminators stripped), removed from
    /// the buffer. A partial trailing line stays put until its newline arrives.
    pub fn lines(&mut self) -> Vec<String> {
        self.buf.lines()
    }

    /// `poll` then `lines`: the lines completed within `timeout`. Empty means
    /// no line completed — NOT necessarily that the port was silent.
    pub fn read_lines(&mut self, timeout: Duration) -> Result<Vec<String>, SerialError> {
        self.poll(timeout)?;
        Ok(self.lines())
    }

    /// The buffered but un-terminated tail, WITHOUT consuming it. This is where
    /// CEFDK's `shell>` prompt sits, since it is printed with no newline.
    pub fn tail(&self) -> String {
        self.buf.tail()
    }

    /// Take the un-terminated tail, clearing it. Only for real silence (a
    /// `poll` that returned `false`): taking it after a read that merely
    /// completed no line throws away the front of a line still arriving.
    pub fn take_pending(&mut self) -> Option<String> {
        self.buf.take_tail()
    }

    /// Drop everything buffered, complete lines and tail alike.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Write a command and the carriage return CEFDK's line editor submits on.
    ///
    /// CEFDK's shell (like most U-Boot-lineage monitors) ends a line on CR, not
    /// LF; sending "\n" leaves the command sitting unexecuted. Flushed so the
    /// byte is on the wire before the caller's inter-command delay starts.
    pub fn write_line(&mut self, cmd: &str) -> Result<(), SerialError> {
        self.port
            .write_all(cmd.as_bytes())
            .and_then(|_| self.port.write_all(b"\r"))
            .and_then(|_| self.port.flush())
            .map_err(|e| SerialError::Io(e.to_string()))
    }
}

/// Console bytes re-assembled into lines, plus the un-terminated tail.
///
/// Pure (no port), so the chunking cases — a line split across reads, a prompt
/// that never gets a newline, a prompt that itself arrives in pieces — are
/// unit-testable, here and by the engine's prompt waits.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    /// Append bytes as they came off the wire.
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// Drain every complete line, leaving the unterminated tail behind.
    pub fn lines(&mut self) -> Vec<String> {
        split_complete_lines(&mut self.pending)
    }

    /// The unterminated tail, left in place: a prompt can be looked for here on
    /// every read without eating the front of a line still arriving.
    pub fn tail(&self) -> String {
        String::from_utf8_lossy(&self.pending).into_owned()
    }

    /// Take the unterminated tail (trimmed), clearing it. `None` if blank.
    pub fn take_tail(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let s = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

/// Drain `buf` of every complete (newline-terminated) line, leaving the
/// unterminated tail behind. Terminators (`\r`, `\n`, `\r\n`) are stripped and
/// empty lines dropped — CEFDK prints `\r\n`, which would otherwise yield a
/// spurious blank line between every real one. Pure, so it is unit-tested.
fn split_complete_lines(buf: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let raw: Vec<u8> = buf.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_matches(['\r', '\n']);
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_split_across_reads_is_reassembled() {
        // The kernel command line is longer than a UART's fill; it arrives in
        // pieces and must come back out as one line, not two.
        let mut buf = b"emmc rd 0x800".to_vec();
        assert!(split_complete_lines(&mut buf).is_empty(), "no newline yet: hold it");
        buf.extend_from_slice(b"400 0x6000000\r\n");
        assert_eq!(split_complete_lines(&mut buf), vec!["emmc rd 0x800400 0x6000000"]);
        assert!(buf.is_empty(), "a fully-consumed line leaves nothing pending");
    }

    #[test]
    fn crlf_does_not_produce_blank_lines_between_real_ones() {
        let mut buf = b"Manufacturing Mode: Enabled\r\nEntering Control4\r\n".to_vec();
        assert_eq!(
            split_complete_lines(&mut buf),
            vec!["Manufacturing Mode: Enabled", "Entering Control4"]
        );
    }

    #[test]
    fn the_unterminated_prompt_stays_pending_then_is_taken() {
        // `shell>` never gets a newline, so it must NOT come out of read's line
        // splitter — and must be recoverable via the pending tail.
        let mut buf = b"banner line\r\nshell>".to_vec();
        assert_eq!(split_complete_lines(&mut buf), vec!["banner line"]);
        assert_eq!(&buf, b"shell>", "the bare prompt is left buffered");
    }

    #[test]
    fn peeking_the_tail_keeps_a_prompt_that_arrives_in_pieces() {
        // Windows' COM driver delivers a few bytes per read. Peeking (not
        // taking) the tail lets `shell>` assemble across reads.
        let mut b = LineBuffer::default();
        b.push(b"File length is 7300096 bytes ( 0x6f6400 in hex )\r\n\r\nsh");
        assert_eq!(b.lines(), vec!["File length is 7300096 bytes ( 0x6f6400 in hex )"]);
        assert!(!b.tail().contains("shell>"));
        b.push(b"ell> ");
        assert!(b.lines().is_empty(), "the prompt never completes a line");
        assert!(b.tail().contains("shell>"), "tail: {:?}", b.tail());
        assert_eq!(b.take_tail().as_deref(), Some("shell>"));
        assert!(b.tail().is_empty());
    }
}
