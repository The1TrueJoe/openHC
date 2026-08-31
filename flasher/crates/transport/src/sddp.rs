//! Control4 SDDP discovery.
//!
//! Control4 controllers answer an SSDP-style multicast search for the
//! `c4:director` service — the very query Control4's own tools use to find a
//! controller. This beats the ARP-by-OUI sweep in two ways: it *positively*
//! identifies a box as a Control4 controller (a freshly restored unit that has
//! dropped off the ARP table still answers here), and it needs no login, so a
//! stock unit shows up before we have any credentials for it.
//!
//! Pure `std` UDP: bind an ephemeral socket, multicast one M-SEARCH, and read
//! the unicast replies for a short window.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// SSDP multicast group and port, and the Control4 service we ask for.
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const PORT: u16 = 1900;
const DIRECTOR: &str = "c4:director";

/// A controller that answered the SDDP search.
#[derive(Debug, Clone)]
pub struct SddpUnit {
    pub ip: String,
    /// Always `None` from SDDP — ARP fills the MAC in. Kept so callers merge
    /// `SddpUnit` and ARP results through one shape.
    pub mac: Option<String>,
}

fn msearch() -> Vec<u8> {
    // MX is the max seconds a device may wait before replying; keep it short so
    // the whole scan stays snappy. The trailing blank line terminates the
    // request, exactly as SSDP requires.
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: {GROUP}:{PORT}\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 2\r\n\
         ST: {DIRECTOR}\r\n\r\n"
    )
    .into_bytes()
}

/// Multicast-search for Control4 directors and collect the responders seen
/// within `wait`. Best-effort: any socket error yields an empty list rather
/// than failing a scan the ARP path can still satisfy.
///
/// ponytail: sends from the default-route interface only. A host multi-homed
/// onto several networks at once would need `socket2`'s `set_multicast_if_v4`
/// to search each; std cannot pick the egress interface. Not worth a dependency
/// until someone actually flashes across two NICs.
pub fn search(wait: Duration) -> Vec<SddpUnit> {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return vec![];
    };
    let _ = sock.set_multicast_ttl_v4(3);
    // A short read timeout so the loop wakes to re-check the deadline; without
    // it a quiet network blocks recv_from forever.
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    if sock.send_to(&msearch(), (GROUP, PORT)).is_err() {
        return vec![];
    }

    let mut out: Vec<SddpUnit> = vec![];
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue; // timeout tick — loop re-checks the deadline
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        // Only Control4 directors, not every UPnP TV on the LAN.
        if !header(&text, "ST").is_some_and(|v| v.contains(DIRECTOR)) {
            continue;
        }
        let SocketAddr::V4(v4) = from else { continue };
        let ip = v4.ip().to_string();
        if out.iter().any(|u| u.ip == ip) {
            continue; // a device may answer more than once
        }
        // MAC is left to ARP, which reads it off the wire reliably; the USN
        // string is not a dependable place to find one.
        out.push(SddpUnit { ip, mac: None });
    }
    out
}

/// Pull one `Name: value` header out of an SSDP reply, case-insensitively.
fn header(msg: &str, name: &str) -> Option<String> {
    msg.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msearch_targets_the_director_service() {
        let m = String::from_utf8(msearch()).unwrap();
        assert!(m.contains("ST: c4:director"));
        assert!(m.starts_with("M-SEARCH * HTTP/1.1"));
        assert!(m.ends_with("\r\n\r\n")); // properly terminated
    }

    #[test]
    fn header_is_case_insensitive() {
        let reply = "HTTP/1.1 200 OK\r\nST: c4:director\r\nUSN: uuid:x\r\n";
        assert_eq!(header(reply, "st").as_deref(), Some("c4:director"));
        assert_eq!(header(reply, "USN").as_deref(), Some("uuid:x"));
        assert!(header(reply, "Location").is_none());
    }

}
