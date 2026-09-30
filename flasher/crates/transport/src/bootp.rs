//! The C4_COOKIE BOOTP responder that drops CEFDK to its shell.
//!
//! In manufacturing mode CEFDK broadcasts a BOOTP request and auto-fetches
//! whatever boot file the reply names. We answer with a reply that (a) carries
//! DHCP option 60 = "C4_COOKIE\0", the vendor-class string CEFDK checks to
//! decide it is on a Control4 provisioning network, and (b) names a BOGUS boot
//! file — so the auto-fetch fails and CEFDK drops to its `shell>`, which is
//! exactly where the netboot takes over.
//!
//! Two facts here were paid for on hardware:
//!   * The cookie CEFDK compares is 10 bytes, "C4_COOKIE\0". Option 60 carries
//!     the 9-char string and is followed by a single PAD (0x00) byte before the
//!     END (0xff) option, so the 10th byte CEFDK reads is the NUL. Put END
//!     straight after the 9 chars and CEFDK reads "C4_COOKIE\xff", rejects it,
//!     and takes its normal boot path (see mfgmode's AC_BOOT handling).
//!   * We answer ONLY the target MAC. On a real network other DHCP traffic is
//!     flying around; replying to everything would hijack unrelated boxes.
//!
//! Binding :67 needs root — expected; the CLI runs under sudo. No privilege
//! dropping.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// BOOTP op codes.
const BOOTREQUEST: u8 = 1;
const BOOTREPLY: u8 = 2;
/// Ethernet: htype 1, 6-byte hardware address.
const HTYPE_ETHER: u8 = 1;
const HLEN_ETHER: u8 = 6;

/// The 4-byte DHCP magic cookie that precedes the options (RFC 1497 / 2131).
const MAGIC: [u8; 4] = [99, 130, 83, 99];
/// Option 60: Vendor Class Identifier. This is where the C4 cookie lives.
const OPT_VENDOR_CLASS: u8 = 60;
/// Option 255: END. Option 0: PAD.
const OPT_END: u8 = 255;
const OPT_PAD: u8 = 0;

/// The vendor-class string CEFDK matches on, without its trailing NUL (the NUL
/// is supplied by a PAD byte after the option — see the module docs).
const COOKIE: &[u8] = b"C4_COOKIE";

/// A deliberately non-existent boot file. Its only job is to make CEFDK's
/// auto-fetch fail so it falls through to the shell; the name never resolves to
/// anything on the TFTP server.
const BOGUS_BOOTFILE: &[u8] = b"openhc-drop-to-shell";

/// A running responder. Dropping it stops the thread; [`stop`](BootpResponder::stop)
/// does so explicitly and joins.
pub struct BootpResponder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BootpResponder {
    /// Bind `0.0.0.0:67` and answer the one `target_mac` with a cookie reply
    /// offering `offer_ip` and naming `server_ip` as the boot server (the box
    /// then talks TFTP to `server_ip`).
    ///
    /// `announce` is called from the thread when the target's request is seen
    /// and answered, so the front end can show the handshake.
    pub fn start(
        target_mac: [u8; 6],
        offer_ip: Ipv4Addr,
        server_ip: Ipv4Addr,
        announce: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<BootpResponder> {
        let sock = UdpSocket::bind(("0.0.0.0", 67))?;
        // The reply goes to the limited broadcast address: the box has no IP yet
        // and cannot be unicast to. A short read timeout lets the loop see stop.
        sock.set_broadcast(true)?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = std::thread::spawn(move || {
            serve_loop(sock, target_mac, offer_ip, server_ip, &stop_thread, &announce)
        });
        Ok(BootpResponder { stop, handle: Some(handle) })
    }

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

impl Drop for BootpResponder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve_loop(
    sock: UdpSocket,
    target_mac: [u8; 6],
    offer_ip: Ipv4Addr,
    server_ip: Ipv4Addr,
    stop: &AtomicBool,
    announce: &(dyn Fn(String) + Send),
) {
    let mut buf = [0u8; 1024];
    while !stop.load(Ordering::SeqCst) {
        let n = match sock.recv_from(&mut buf) {
            Ok((n, _from)) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue
            }
            Err(_) => continue,
        };
        let Some(req) = parse_bootrequest(&buf[..n]) else { continue };
        // Only ours: someone else's DHCP request is not our business.
        if req.mac != target_mac {
            continue;
        }
        announce(format!("bootp: request from {}", mac_str(&req.mac)));
        let reply = build_cookie_reply(&req.xid, &req.mac, offer_ip, server_ip);
        // The client listens for the reply on :68 and has no address yet, so
        // broadcast it.
        match sock.send_to(&reply, (Ipv4Addr::BROADCAST, 68)) {
            Ok(_) => announce(format!(
                "bootp: answered {} with C4_COOKIE (offer {offer_ip}, server {server_ip})",
                mac_str(&req.mac)
            )),
            Err(e) => announce(format!("bootp: reply send failed: {e}")),
        }
    }
}

/// The fields of a BOOTREQUEST the responder needs.
pub struct BootRequest {
    pub xid: [u8; 4],
    pub mac: [u8; 6],
}

/// Parse a BOOTREQUEST, returning its xid and client MAC. None for anything
/// that is not an Ethernet BOOTP request. Pure — unit-tested.
///
/// Fixed BOOTP header offsets: op(0), htype(1), hlen(2), hops(3), xid(4..8),
/// ... , chaddr(28..44, first 6 bytes are the MAC for Ethernet).
pub fn parse_bootrequest(pkt: &[u8]) -> Option<BootRequest> {
    if pkt.len() < 44 {
        return None;
    }
    if pkt[0] != BOOTREQUEST || pkt[1] != HTYPE_ETHER || pkt[2] != HLEN_ETHER {
        return None;
    }
    let mut xid = [0u8; 4];
    xid.copy_from_slice(&pkt[4..8]);
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&pkt[28..34]);
    Some(BootRequest { xid, mac })
}

/// Build the BOOTREPLY carrying the C4 cookie. Pure — unit-tested.
///
/// The BOOTP header is 236 bytes fixed, then the magic cookie, then the
/// options. `offer_ip` is what CEFDK takes as its address (yiaddr); `server_ip`
/// is the TFTP host it then talks to (siaddr).
pub fn build_cookie_reply(
    xid: &[u8; 4],
    mac: &[u8; 6],
    offer_ip: Ipv4Addr,
    server_ip: Ipv4Addr,
) -> Vec<u8> {
    let mut p = vec![0u8; 236];
    p[0] = BOOTREPLY;
    p[1] = HTYPE_ETHER;
    p[2] = HLEN_ETHER;
    // hops(3)=0, secs(8..10)=0, flags(10..12)=0, ciaddr(12..16)=0 all left zero.
    p[4..8].copy_from_slice(xid);
    p[16..20].copy_from_slice(&offer_ip.octets()); // yiaddr — the box's address
    p[20..24].copy_from_slice(&server_ip.octets()); // siaddr — the boot server
    // giaddr(24..28)=0. chaddr(28..44): the client MAC in the first 6 bytes.
    p[28..34].copy_from_slice(mac);
    // sname(44..108) and file(108..236) zeroed, then the bogus boot filename so
    // CEFDK's auto-fetch fails and it drops to the shell.
    let file = &mut p[108..236];
    file[..BOGUS_BOOTFILE.len()].copy_from_slice(BOGUS_BOOTFILE);

    // Magic cookie, then option 60, then a PAD NUL (the cookie's 10th byte),
    // then END. See the module docs for why the PAD is not optional.
    p.extend_from_slice(&MAGIC);
    p.push(OPT_VENDOR_CLASS);
    p.push(COOKIE.len() as u8);
    p.extend_from_slice(COOKIE);
    p.push(OPT_PAD);
    p.push(OPT_END);
    p
}

/// `aa:bb:cc:dd:ee:ff` -> six bytes. Accepts `:` or `-` separators and any
/// case. None on anything malformed. Pure — unit-tested.
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part.trim(), 16).ok()?;
    }
    Some(mac)
}

fn mac_str(mac: &[u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_reply() -> Vec<u8> {
        build_cookie_reply(
            &[0xde, 0xad, 0xbe, 0xef],
            &[0x00, 0x1b, 0x22, 0x33, 0x44, 0x55],
            Ipv4Addr::new(10, 0, 0, 200),
            Ipv4Addr::new(10, 0, 0, 106),
        )
    }

    #[test]
    fn reply_is_a_bootreply_for_ethernet() {
        let p = sample_reply();
        assert_eq!(p[0], BOOTREPLY, "op must be 2 (reply)");
        assert_eq!(p[1], HTYPE_ETHER);
        assert_eq!(p[2], HLEN_ETHER);
    }

    #[test]
    fn xid_and_mac_are_echoed_back() {
        let p = sample_reply();
        assert_eq!(&p[4..8], &[0xde, 0xad, 0xbe, 0xef], "xid must match the request");
        assert_eq!(&p[28..34], &[0x00, 0x1b, 0x22, 0x33, 0x44, 0x55], "chaddr = client MAC");
    }

    #[test]
    fn yiaddr_is_the_offer_and_siaddr_is_the_server() {
        let p = sample_reply();
        assert_eq!(&p[16..20], &[10, 0, 0, 200], "yiaddr = offered IP");
        assert_eq!(&p[20..24], &[10, 0, 0, 106], "siaddr = TFTP server");
    }

    #[test]
    fn the_magic_cookie_is_present_at_the_start_of_options() {
        let p = sample_reply();
        assert_eq!(&p[236..240], &MAGIC, "options begin with the DHCP magic cookie");
    }

    /// The whole reason this responder exists: the cookie bytes CEFDK reads must
    /// be exactly "C4_COOKIE\0". This pins option 60 = the 9 chars, immediately
    /// followed by a PAD NUL, then END — the layout that was accepted on
    /// hardware, and the one whose absence (END straight after) was rejected.
    #[test]
    fn option_60_carries_the_cookie_with_a_trailing_nul_before_end() {
        let p = sample_reply();
        let opts = &p[240..];
        assert_eq!(opts[0], OPT_VENDOR_CLASS, "first option is vendor class (60)");
        assert_eq!(opts[1] as usize, COOKIE.len(), "length is the 9-char cookie");
        assert_eq!(&opts[2..2 + COOKIE.len()], COOKIE, "the cookie string");
        let after = 2 + COOKIE.len();
        assert_eq!(opts[after], OPT_PAD, "a NUL (PAD) is the cookie's 10th byte");
        assert_eq!(opts[after + 1], OPT_END, "then END — not straight after the 9 chars");
    }

    #[test]
    fn a_bogus_boot_filename_is_set_so_autofetch_fails() {
        let p = sample_reply();
        assert_eq!(&p[108..108 + BOGUS_BOOTFILE.len()], BOGUS_BOOTFILE);
    }

    #[test]
    fn parse_bootrequest_reads_xid_and_mac() {
        let mut pkt = vec![0u8; 44];
        pkt[0] = BOOTREQUEST;
        pkt[1] = HTYPE_ETHER;
        pkt[2] = HLEN_ETHER;
        pkt[4..8].copy_from_slice(&[1, 2, 3, 4]);
        pkt[28..34].copy_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let req = parse_bootrequest(&pkt).expect("valid request");
        assert_eq!(req.xid, [1, 2, 3, 4]);
        assert_eq!(req.mac, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    }

    #[test]
    fn parse_bootrequest_rejects_non_requests() {
        let mut reply = vec![0u8; 44];
        reply[0] = BOOTREPLY; // a reply, not a request
        reply[1] = HTYPE_ETHER;
        reply[2] = HLEN_ETHER;
        assert!(parse_bootrequest(&reply).is_none(), "must not answer another server's reply");
        assert!(parse_bootrequest(&[0u8; 10]).is_none(), "too short");
    }

    #[test]
    fn mac_parses_both_separators_and_any_case() {
        assert_eq!(parse_mac("00:1b:22:33:44:55"), Some([0x00, 0x1b, 0x22, 0x33, 0x44, 0x55]));
        assert_eq!(parse_mac("AA-BB-CC-DD-EE-FF"), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(parse_mac("001b22334455"), None, "no separators is not a MAC");
        assert_eq!(parse_mac("00:1b:22:33:44"), None, "five octets is not a MAC");
        assert_eq!(parse_mac("zz:1b:22:33:44:55"), None, "non-hex is rejected");
    }
}
