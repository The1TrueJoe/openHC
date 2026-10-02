//! Serial transport for driving the CEFDK console.
//!
//! The EA netboot bring-up types commands at CEFDK's `shell>` and watches its
//! output. That is line-oriented at 115200 8N1, so this wraps the raw port in a
//! tiny type that gives the engine two operations: pull whatever COMPLETE lines
//! have arrived (to feed the pure `MfgWatch` state machine) and write a command
//! terminated the way the shell expects.
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
/// Reads are chunked off the wire and re-assembled into lines here, because a
/// UART hands you bytes whenever they arrive — a single console line routinely
/// straddles two reads, and a command echoed back can arrive a character at a
/// time. Splitting on the terminator and buffering the tail is what turns that
/// stream back into the lines `MfgWatch` was written against.
pub struct Serial {
    port: Box<dyn serialport::SerialPort>,
    /// Bytes read but not yet terminated by a newline — the tail of a line
    /// still coming in.
    pending: Vec<u8>,
    pub dev: String,
    pub baud: u32,
}

impl Serial {
    /// Open `dev` at `baud`, 8N1. The per-read timeout is set fresh on every
    /// `read_lines` call, so the value passed here is only a placeholder.
    pub fn open(dev: &str, baud: u32) -> Result<Serial, SerialError> {
        let port = serialport::new(dev, baud)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| SerialError::Open { dev: dev.to_string(), msg: e.to_string() })?;
        Ok(Serial { port, pending: Vec::new(), dev: dev.to_string(), baud })
    }

    /// Read whatever bytes are available within `timeout` and return the
    /// COMPLETE lines they finished (terminator stripped). A partial trailing
    /// line stays buffered until its newline arrives, so a command split across
    /// two reads is never fed as two half-lines. An empty result means the
    /// window passed with no line completed — the caller treats that as silence.
    pub fn read_lines(&mut self, timeout: Duration) -> Result<Vec<String>, SerialError> {
        self.port
            .set_timeout(timeout)
            .map_err(|e| SerialError::Io(e.to_string()))?;
        let mut buf = [0u8; 512];
        match self.port.read(&mut buf) {
            Ok(n) => self.pending.extend_from_slice(&buf[..n]),
            // A timeout is the normal "nothing arrived this window" case, not an
            // error. Some platforms report an empty read the same way.
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(SerialError::Io(e.to_string())),
        }
        Ok(split_complete_lines(&mut self.pending))
    }

    /// Take any buffered but un-terminated text, clearing it.
    ///
    /// The CEFDK prompt `shell>` is printed WITHOUT a trailing newline — it is
    /// waiting for input — so it never completes a line and `read_lines` alone
    /// would sit on it forever. On a silent read the caller pulls the tail with
    /// this and feeds it once, which is how the prompt gets seen. Feeding a
    /// genuine mid-line fragment this way at worst splits one `Observed` log
    /// line; milestone detection is by substring and unaffected.
    pub fn take_pending(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let s = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
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
}
