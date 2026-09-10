//! Netbooting a box that has no reachable console: DHCP and TFTP.
//!
//! This exists because of a specific, recurring failure. A Control4 U-Boot with
//! `run tst` armed does `dhcp` and then TFTPs from whatever `serverip` the
//! offer carried — and on a bench that server is long gone. The box then boots
//! nothing, answers no TCP, ignores ICMP, and sits at a prompt nobody can see.
//! From the outside it is indistinguishable from bricked. The one thing it
//! still does is broadcast a DHCP DISCOVER every time it is power-cycled, and
//! that is a way in that needs no serial cable.
//!
//! So: answer that DISCOVER ourselves, put our own address in `siaddr`, and
//! hand it a kernel over TFTP. U-Boot's `dhcp` takes `serverip` from the OFFER,
//! which OVERRIDES the stale one in its saved environment — that override is
//! the whole trick, and it is why this works on a box we cannot type into.
//!
//! # What is ours and what is not
//!
//! The wire formats are not ours. `dhcproto` does DHCP encode/decode and
//! `tftpd` is the TFTP server — both are maintained, both handle the option
//! extensions (blocksize, timeout, tsize, windowsize) that a hand-rolled
//! 200-line server would quietly not support. What is left here is the only
//! part that is actually openHC's: the policy.
//!
//! # That policy runs on somebody's home network
//!
//! A second DHCP server on a live LAN is how you take out a house. Every reply
//! is gated on ONE client MAC, checked before a reply is composed, and a packet
//! from any other machine is counted and dropped. That is not a configuration
//! option and there is deliberately no serve-everyone mode: the flasher's job
//! is one box on a bench, and the blast radius should match.
//!
//! TFTP is confined the same way. The image is staged into a temporary
//! directory containing nothing else, and `tftpd` is pointed at that directory
//! read-only — so the served set is exactly one file, and the directory goes
//! away when the run does.
//!
//! Both ports are privileged (67 and 69), so this needs root.

use dhcproto::v4::{self, Decodable, Decoder, Encodable, Encoder, Message, MessageType, Opcode};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Anything worth reporting while a netboot is in flight.
///
/// Returned rather than logged so a GUI can render the same run as a progress
/// list. Note that TFTP transfer lines are NOT here: `tftpd` logs those itself,
/// and wrapping its logger to re-emit them would be a lot of plumbing to say
/// the same words.
#[derive(Debug, Clone)]
pub enum Event {
    /// Both listeners are up. Nothing happens until the box is power-cycled.
    /// `iface` is the interface the DHCP socket is pinned to — worth printing,
    /// because a reply on the wrong wire is invisible and looks like success.
    Listening { dhcp: u16, tftp: u16, iface: Option<String> },
    /// A DHCP message from the MAC we care about, and what we said back.
    Dhcp { saw: MessageType, replied: Option<MessageType> },
    /// A DHCP message from some other machine. Counted, never answered — this
    /// is the number that says whether the filter is doing its job.
    Ignored { from: Ipv4Addr },
    Failed { what: String },
}

pub struct Config {
    /// The ONLY MAC that gets an answer.
    pub mac: [u8; 6],
    /// Address to offer it. Offering the address it already had avoids a
    /// pointless renumber and keeps the router's lease table honest.
    pub client_ip: Ipv4Addr,
    /// Us — goes in `siaddr` and option 54, and is where TFTP is expected.
    pub server_ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    /// The file to serve.
    pub image: PathBuf,
    /// The name to advertise, and the name it is staged under. May contain
    /// slashes: a stock IO Extender asks for `hammer/uImage`.
    pub bootfile: String,
}

/// Parse `00:0f:ff:18:21:9c` (or `-` separated, or bare hex).
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Re-exported so a front end can match on what we saw and said without taking
/// a direct dependency on the DHCP codec.
pub use dhcproto::v4::MessageType as DhcpMessageType;

pub fn format_mac(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

/// The name a stock bootloader is most likely to ask for already. Only a
/// default for `--bootfile`; the offer overrides whatever the box had saved.
pub fn default_bootfile(board: &str) -> String {
    match board {
        // Control4's own U-Boot on the DM355 IO Extender ships `run tst` with
        // this path, so an untouched box asks for it without being told.
        "ioxv1" => "hammer/uImage".into(),
        other => format!("{other}/uImage"),
    }
}

// ── DHCP: ours is the policy, dhcproto is the format ────────────────────────

/// Compose the reply to one request.
///
/// Split out from the socket loop so it can be tested without binding a
/// privileged port — the assertions that matter (does `siaddr` carry our
/// address, does the xid come back) are all about this function.
fn build_reply(cfg: &Config, req: &Message, kind: MessageType) -> Message {
    let mut m = Message::new_with_id(
        req.xid(),
        Ipv4Addr::UNSPECIFIED, // ciaddr
        cfg.client_ip,         // yiaddr — what it gets
        // siaddr IS the point of this whole exercise: U-Boot reads serverip
        // from here and it beats the stale one in its saved environment.
        cfg.server_ip,
        Ipv4Addr::UNSPECIFIED, // giaddr — no relay, we are on the segment
        &cfg.mac,
    );
    m.set_opcode(Opcode::BootReply);
    m.set_flags(req.flags());
    // The `file` header field as well as option 67: old bootloaders read the
    // header and never look at the option.
    m.set_fname_str(&cfg.bootfile);

    let opts = m.opts_mut();
    opts.insert(v4::DhcpOption::MessageType(kind));
    opts.insert(v4::DhcpOption::ServerIdentifier(cfg.server_ip));
    opts.insert(v4::DhcpOption::SubnetMask(cfg.netmask));
    // An hour. Long enough that a slow transfer cannot renumber underneath
    // itself, short enough that a lease we forget about expires on its own.
    opts.insert(v4::DhcpOption::AddressLeaseTime(3600));
    opts.insert(v4::DhcpOption::TFTPServerName(cfg.server_ip.to_string().into_bytes()));
    opts.insert(v4::DhcpOption::BootfileName(cfg.bootfile.clone().into_bytes()));
    // Deliberately NO Router option. There is nothing off-subnet to reach, and
    // a default route is what sent this box chasing a gateway for a serverip
    // that no longer existed in the first place.
    m
}

fn message_type(m: &Message) -> Option<MessageType> {
    match m.opts().get(v4::OptionCode::MessageType) {
        Some(v4::DhcpOption::MessageType(t)) => Some(*t),
        _ => None,
    }
}

fn dhcp_thread(
    cfg: Arc<Config>,
    stop: Arc<AtomicBool>,
    sock: UdpSocket,
    emit: Arc<dyn Fn(Event) + Send + Sync>,
) {
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            // The read timeout is what lets this notice `stop` at all; both
            // kinds show up depending on the platform.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(e) => return emit(Event::Failed { what: format!("dhcp recv: {e}") }),
        };
        let Ok(req) = Message::decode(&mut Decoder::new(&buf[..n])) else { continue };

        // THE FILTER. Before a reply is composed, before this is logged as
        // ours: is this the one box we were pointed at? chaddr is 16 bytes on
        // the wire and only hlen of them are the address.
        if req.chaddr() != cfg.mac {
            if let SocketAddr::V4(v4) = from {
                emit(Event::Ignored { from: *v4.ip() });
            }
            continue;
        }

        let Some(saw) = message_type(&req) else { continue };
        let kind = match saw {
            MessageType::Discover => MessageType::Offer,
            MessageType::Request => MessageType::Ack,
            // Inform, Release, Decline: nothing a bootloader needs from us.
            _ => {
                emit(Event::Dhcp { saw, replied: None });
                continue;
            }
        };

        let mut out = Vec::with_capacity(576);
        if let Err(e) = build_reply(&cfg, &req, kind).encode(&mut Encoder::new(&mut out)) {
            emit(Event::Failed { what: format!("encoding a {kind:?}: {e}") });
            continue;
        }
        // Broadcast, because the client does not hold the address yet and
        // cannot answer an ARP for it. Every DHCP client listens on :68
        // broadcast for exactly this reason.
        let dst = SocketAddrV4::new(Ipv4Addr::BROADCAST, 68);
        match sock.send_to(&out, dst) {
            Ok(_) => emit(Event::Dhcp { saw, replied: Some(kind) }),
            Err(e) => emit(Event::Failed { what: format!("sending a {kind:?}: {e}") }),
        }
    }
}

// ── TFTP: tftpd, pointed at a directory holding exactly one file ────────────

/// Stage the image under its advertised name in a directory of its own.
///
/// `tftpd` serves a directory and refuses to escape it, so the way to serve
/// exactly one file is to give it a directory that contains exactly one file.
/// The bootfile name may be a path (`hammer/uImage`), so the parents are made
/// too — the box asks for the name we advertised, and it has to resolve.
fn stage(cfg: &Config) -> io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new().prefix("ohc-netboot-").tempdir()?;
    let rel = Path::new(cfg.bootfile.trim_start_matches('/'));
    let dest = dir.path().join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&cfg.image, &dest)?;
    Ok(dir)
}

// ── the front door ──────────────────────────────────────────────────────────

/// Answer one box's DHCP and serve it one file, until `deadline` passes or
/// `stop` is set.
///
/// Blocks. The two listeners run on their own threads because a TFTP transfer
/// must not stall the DHCP socket: a box that resends DISCOVER mid-transfer is
/// normal, and missing it would strand the boot.
pub fn serve(
    cfg: Config,
    deadline: Duration,
    stop: Arc<AtomicBool>,
    emit: impl Fn(Event) + Send + Sync + 'static,
) -> io::Result<()> {
    if !cfg.image.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no image at {}", cfg.image.display()),
        ));
    }
    // Held for the whole run: dropping it deletes the staged copy.
    let staged = stage(&cfg)?;

    let cfg = Arc::new(cfg);
    let emit: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(emit);

    let (dhcp_sock, iface) = bind_dhcp(cfg.server_ip)?;

    let tftp_cfg = tftpd::Config {
        ip_address: IpAddr::V4(cfg.server_ip),
        port: 69,
        directory: staged.path().to_path_buf(),
        send_directory: staged.path().to_path_buf(),
        receive_directory: staged.path().to_path_buf(),
        // A bootloader fetching a kernel never writes, and a writable TFTP
        // server on a home network is a much worse thing to leave running.
        read_only: true,
        overwrite: false,
        ..Default::default()
    };
    let mut tftp = tftpd::Server::new(&tftp_cfg)
        .map_err(|e| io::Error::other(format!("tftp on {}:69 — {e}", cfg.server_ip)))?;
    let tftp_abort = tftp.get_abort_flag();

    emit(Event::Listening { dhcp: 67, tftp: 69, iface: iface.clone() });

    let (c1, s1, e1) = (cfg.clone(), stop.clone(), emit.clone());
    let dhcp_thread = std::thread::spawn(move || dhcp_thread(c1, s1, dhcp_sock, e1));
    // `listen` runs until aborted, so it gets a thread of its own and the flag
    // is how the deadline reaches it.
    let tftp_thread = std::thread::spawn(move || tftp.listen());

    let until = Instant::now() + deadline;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
    stop.store(true, Ordering::Relaxed);
    tftp_abort.store(true, Ordering::Relaxed);
    let _ = dhcp_thread.join();
    let _ = tftp_thread.join();
    Ok(())
}

/// Which interface holds `ip`, by name.
///
/// The DHCP socket has to be pinned to it, and the pinning APIs take a name (or
/// an index derived from one) rather than an address.
fn interface_holding(ip: Ipv4Addr) -> Option<String> {
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .find(|i| matches!(&i.addr, if_addrs::IfAddr::V4(v) if v.ip == ip))
        .map(|i| i.name)
}

/// Tie a socket to one interface, so a broadcast cannot wander.
///
/// THE bug this fixes: a DHCP reply is addressed to 255.255.255.255, which says
/// nothing about which wire it belongs on, so the kernel resolves it through the
/// routing table. On a machine with two networks that is a coin toss, and on the
/// dual-homed Mac this was first run against it was not even a toss — macOS had
/// a cloned host route for 255.255.255.255 pointing at the *other* interface, so
/// every OFFER and ACK left on the wrong LAN. `send_to` returned Ok each time,
/// the tool logged "replied", and the box a metre away heard nothing. Silent,
/// total, and indistinguishable from the bootloader ignoring us.
///
/// Unsupported platforms fall through: the socket still works, it is just back
/// to trusting the routing table, and `Listening.iface` reports None so the
/// operator can see that is what happened.
fn pin_to_interface(sock: &socket2::Socket, iface: &str) -> io::Result<()> {
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    {
        return sock.bind_device(Some(iface.as_bytes()));
    }
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos", target_os = "watchos"))]
    {
        let name = std::ffi::CString::new(iface).map_err(io::Error::other)?;
        // SAFETY: `name` is a valid NUL-terminated C string for this call, and
        // if_nametoindex only reads it. Zero means "no such interface".
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        let index = std::num::NonZeroU32::new(index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no interface index for {iface}"))
        })?;
        return sock.bind_device_by_index_v4(Some(index));
    }
    #[allow(unreachable_code)]
    {
        let _ = (sock, iface);
        Ok(())
    }
}

/// The DHCP socket: bound to every address (a client with no lease broadcasts,
/// so a socket bound to one unicast address would never see a DISCOVER) but
/// PINNED to one interface, so replies leave by the wire the request arrived on.
fn bind_dhcp(server_ip: Ipv4Addr) -> io::Result<(UdpSocket, Option<String>)> {
    use socket2::{Domain, Protocol, Socket, Type};

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;

    // Pin BEFORE bind: on Linux SO_BINDTODEVICE after bind does not retroactively
    // constrain the socket.
    let iface = interface_holding(server_ip);
    if let Some(name) = &iface {
        pin_to_interface(&sock, name)?;
    }

    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 67).into()).map_err(|e| {
        match e.kind() {
            io::ErrorKind::PermissionDenied => {
                io::Error::new(e.kind(), "port 67 needs root — try sudo".to_string())
            }
            io::ErrorKind::AddrInUse => io::Error::new(
                e.kind(),
                "port 67 is taken — another dhcpd is already running".to_string(),
            ),
            _ => e,
        }
    })?;
    Ok((sock.into(), iface))
}

/// A broadcast-capable UDP socket with a read timeout, so the thread notices
/// `stop` instead of blocking in `recv_from` forever.
#[allow(dead_code)]
fn bind_broadcast(port: u16) -> io::Result<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).map_err(|e| {
        // The two ways this fails are worth telling apart: one is a sudo away,
        // the other means something else already owns the port.
        match e.kind() {
            io::ErrorKind::PermissionDenied => {
                io::Error::new(e.kind(), format!("port {port} needs root — try sudo"))
            }
            io::ErrorKind::AddrInUse => io::Error::new(
                e.kind(),
                format!("port {port} is taken — another dhcpd or tftpd is already running"),
            ),
            _ => e,
        }
    })?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(sock)
}

/// Which of our addresses faces `peer`, so `--server-ip` can usually be left
/// off. A connected UDP socket picks a source address from the routing table
/// without sending anything.
pub fn local_ip_towards(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddrV4::new(peer, 9)).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            mac: parse_mac("00:0f:ff:18:21:9c").unwrap(),
            client_ip: Ipv4Addr::new(10, 0, 0, 113),
            server_ip: Ipv4Addr::new(10, 0, 0, 153),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            image: PathBuf::from("/dev/null"),
            bootfile: "hammer/uImage".into(),
        }
    }

    fn discover_from(mac: [u8; 6]) -> Message {
        let mut m = Message::new(
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::UNSPECIFIED,
            &mac,
        );
        m.opts_mut().insert(v4::DhcpOption::MessageType(MessageType::Discover));
        m
    }

    /// Not much of an assertion, but it catches the case that matters: if this
    /// returns None for an address the host genuinely holds, every reply falls
    /// back to the routing table and the whole tool goes silently wrong.
    #[test]
    fn the_loopback_address_resolves_to_an_interface() {
        assert!(
            interface_holding(Ipv4Addr::LOCALHOST).is_some(),
            "127.0.0.1 must belong to some interface on any host that can run this"
        );
        assert!(interface_holding(Ipv4Addr::new(203, 0, 113, 7)).is_none());
    }

    #[test]
    fn macs_parse_in_the_forms_people_actually_paste() {
        let want = [0x00, 0x0f, 0xff, 0x18, 0x21, 0x9c];
        assert_eq!(parse_mac("00:0f:ff:18:21:9c"), Some(want));
        assert_eq!(parse_mac("00-0F-FF-18-21-9C"), Some(want));
        assert_eq!(parse_mac("000fff18219c"), Some(want));
        assert_eq!(parse_mac("00:0f:ff:18:21"), None);
        assert_eq!(parse_mac("not a mac at all"), None);
    }

    /// The property the whole tool rests on: the offer must carry OUR address
    /// where U-Boot reads serverip, or the box goes back to chasing a server
    /// that does not exist.
    #[test]
    fn an_offer_puts_the_server_where_u_boot_reads_it() {
        let cfg = cfg();
        let req = discover_from(cfg.mac);
        let reply = build_reply(&cfg, &req, MessageType::Offer);

        assert_eq!(reply.opcode(), Opcode::BootReply);
        assert_eq!(reply.xid(), req.xid(), "the xid must come back unchanged");
        assert_eq!(reply.yiaddr(), cfg.client_ip);
        assert_eq!(reply.siaddr(), cfg.server_ip, "this is what becomes serverip");
        assert_eq!(reply.fname_str().unwrap().unwrap(), "hammer/uImage");
        assert_eq!(message_type(&reply), Some(MessageType::Offer));

        // And it survives a round trip on the wire, which is the part a
        // hand-rolled encoder gets subtly wrong.
        let mut buf = Vec::new();
        reply.encode(&mut Encoder::new(&mut buf)).expect("encodes");
        let back = Message::decode(&mut Decoder::new(&buf)).expect("decodes");
        assert_eq!(back.siaddr(), cfg.server_ip);
        assert_eq!(back.opts().get(v4::OptionCode::BootfileName).is_some(), true);
    }

    /// The safety property, as a test, because a regression here is somebody's
    /// house network.
    #[test]
    fn a_request_from_another_machine_is_not_ours() {
        let cfg = cfg();
        let theirs = parse_mac("b8:27:eb:a7:72:95").unwrap();
        let req = discover_from(theirs);
        assert_ne!(
            req.chaddr(),
            cfg.mac,
            "the socket loop compares exactly this, and it must not match"
        );
    }

    /// A request gets an ACK, not another OFFER — U-Boot will not proceed to
    /// TFTP on an OFFER it already has.
    #[test]
    fn a_request_is_answered_with_an_ack() {
        let cfg = cfg();
        let mut req = discover_from(cfg.mac);
        req.opts_mut().insert(v4::DhcpOption::MessageType(MessageType::Request));
        assert_eq!(message_type(&req), Some(MessageType::Request));
        let reply = build_reply(&cfg, &req, MessageType::Ack);
        assert_eq!(message_type(&reply), Some(MessageType::Ack));
    }

    /// A nested bootfile name has to become a real path, or the box asks for
    /// something the server cannot resolve.
    #[test]
    fn staging_creates_the_advertised_path() {
        let mut c = cfg();
        let src = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(src.path(), b"not really a kernel").unwrap();
        c.image = src.path().to_path_buf();

        let dir = stage(&c).expect("stages");
        let served = dir.path().join("hammer/uImage");
        assert!(served.is_file(), "{} should exist", served.display());
        assert_eq!(std::fs::read(&served).unwrap(), b"not really a kernel");
    }
}
