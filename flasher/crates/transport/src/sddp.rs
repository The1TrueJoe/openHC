//! Control4 discovery over multicast — both dialects, because they carry
//! different halves of the answer.
//!
//! **udp/1900, `ST: c4:director`.** Controllers announce themselves here in an
//! SSDP-shaped message. The useful field is the USN, which is not a uuid but a
//! readable identity:
//!
//! ```text
//! NOTIFY * HTTP/1.1
//! ST: c4:director
//! USN: c4:director:Living-EA1-000FFF9163CB
//! CACHE-CONTROL: max-age = 5000, no-cache="Ext"
//! ```
//!
//! That trailing hex is the MAC. This module used to throw the whole USN away
//! with a comment saying it was "not a dependable place to find one".
//!
//! **udp/1902, SDDP proper.** Devices — including controllers — announce with a
//! full record:
//!
//! ```text
//! NOTIFY ALIVE SDDP/1.0
//! From: "192.168.1.109:1902"
//! Host: "leaf_ultra_lu862-70b3d50c1637"
//! Type: "c4:leaf_ultra_hdmivswitch_c"
//! Manufacturer: "Leaf Audio"
//! Model: "ULTRA LU862"
//! Driver: "leaf_ultra_lu862_ip.c4i"
//! Config-URL: "http://192.168.1.109"
//! ```
//!
//! WHAT SDDP DOES NOT CARRY IS A FIRMWARE VERSION. Model, type, manufacturer
//! and driver, yes; an OS version, no — on any of the units measured here. A
//! version has to be asked for over HTTP or SSH once a unit is found, so this
//! module does not pretend to supply one.
//!
//! ANNOUNCEMENTS ARE PERIODIC AND RARE. The EA1 above advertises `max-age =
//! 5000`, i.e. roughly once an hour and a half, and controllers do not reliably
//! answer an active search. So a short scan legitimately sees nothing, and the
//! ARP-by-OUI path in `discovery` is what actually finds a quiet controller.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const SDDP_PORT: u16 = 1902;
const DIRECTOR: &str = "c4:director";

/// A device that announced itself. Everything past `ip` is best-effort: which
/// fields arrive depends on which dialect the device speaks.
#[derive(Debug, Clone, Default)]
pub struct SddpUnit {
    pub ip: String,
    pub mac: Option<String>,
    /// `Living-EA1` from a director USN, or the SDDP `Host` field.
    pub name: Option<String>,
    /// SDDP `Type`, e.g. `c4:controller_hc800`.
    pub kind: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub driver: Option<String>,
    pub config_url: Option<String>,
    /// True when it answered the `c4:director` search — i.e. it is a controller
    /// running Director, not merely a device Control4 can drive.
    pub is_director: bool,
}

fn ssdp_msearch() -> Vec<u8> {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: {GROUP}:{SSDP_PORT}\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 2\r\n\
         ST: {DIRECTOR}\r\n\r\n"
    )
    .into_bytes()
}

/// SDDP's own search. `Host` must be the sender's address, not a hostname —
/// devices reply to it directly.
fn sddp_search(from: Ipv4Addr) -> Vec<u8> {
    format!("SEARCH * SDDP/1.0\r\nHost: \"{from}:{SDDP_PORT}\"\r\n\r\n").into_bytes()
}

/// Search for devices and collect what answers within `wait`.
///
/// `also_ask` is a list of addresses to query DIRECTLY, in addition to the
/// multicast search.
///
/// MULTICAST ONLY LEAVES ONE INTERFACE, and `std`'s `UdpSocket` cannot choose
/// which (there is no `set_multicast_if_v4`; that needs `socket2`, and this
/// crate is deliberately dependency-free). On a machine with more than one
/// network — the normal case for anyone whose controllers are on a separate
/// subnet, and true of the machine this was developed on, which answers on four
/// addresses — that silently misses every unit off the default route.
///
/// The way round it costs nothing: these devices answer a UNICAST search just
/// as well as a multicast one, and unicast is routed per-destination by the
/// kernel with no socket options at all. `discovery` already knows which
/// addresses hold a Control4 OUI, so it hands them in here and each is asked
/// directly. Multicast then only has to catch what ARP did not already see.
pub fn search_from(also_ask: &[Ipv4Addr], wait: Duration) -> Vec<SddpUnit> {
    let mut units: BTreeMap<String, SddpUnit> = BTreeMap::new();
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return vec![];
    };
    let _ = sock.set_multicast_ttl_v4(4);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));

    let local = sock.local_addr().ok().map(|a| a.ip()).and_then(|ip| match ip {
        std::net::IpAddr::V4(v4) => Some(v4),
        _ => None,
    }).unwrap_or(Ipv4Addr::UNSPECIFIED);

    let _ = sock.send_to(&ssdp_msearch(), (GROUP, SSDP_PORT));
    let _ = sock.send_to(&sddp_search(local), (GROUP, SDDP_PORT));
    for ip in also_ask {
        let _ = sock.send_to(&ssdp_msearch(), (*ip, SSDP_PORT));
        let _ = sock.send_to(&sddp_search(*ip), (*ip, SDDP_PORT));
    }

    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        let SocketAddr::V4(v4) = from else { continue };
        let text = String::from_utf8_lossy(&buf[..n]);
        if let Some(u) = parse(&text, &v4.ip().to_string()) {
            // Merge rather than replace: a unit may answer on both dialects,
            // and each carries fields the other does not.
            let e = units.entry(u.ip.clone()).or_insert_with(|| SddpUnit {
                ip: u.ip.clone(),
                ..Default::default()
            });
            e.mac = e.mac.take().or(u.mac);
            e.name = e.name.take().or(u.name);
            e.kind = e.kind.take().or(u.kind);
            e.manufacturer = e.manufacturer.take().or(u.manufacturer);
            e.model = e.model.take().or(u.model);
            e.driver = e.driver.take().or(u.driver);
            e.config_url = e.config_url.take().or(u.config_url);
            e.is_director |= u.is_director;
        }
    }
    units.into_values().collect()
}

/// Multicast only — no address list to ask directly.
pub fn search(wait: Duration) -> Vec<SddpUnit> {
    search_from(&[], wait)
}

/// Turn one datagram into a unit, or None if it is not Control4's.
///
/// Pure, so the two message shapes can be tested without a controller on the
/// bench — which matters, because they only appear every ~80 minutes.
pub fn parse(text: &str, from_ip: &str) -> Option<SddpUnit> {
    let mut u = SddpUnit { ip: from_ip.to_string(), ..Default::default() };

    // Our own outgoing searches come back to us on the multicast group; they
    // are not discoveries and listing them would report the operator's own
    // machine as a controller.
    if text.starts_with("M-SEARCH") || text.starts_with("SEARCH ") {
        return None;
    }

    if text.contains("SDDP/1.0") {
        // `Host: "name"` — quoted, and the quotes are part of the wire format.
        u.name = field(text, "Host");
        u.kind = field(text, "Type");
        u.manufacturer = field(text, "Manufacturer");
        u.model = field(text, "Model");
        u.driver = field(text, "Driver");
        u.config_url = field(text, "Config-URL");
        // OFFLINE is a device saying goodbye; do not list it as present.
        if text.starts_with("NOTIFY OFFLINE") {
            return None;
        }
        u.is_director = u.kind.as_deref().is_some_and(|k| k.contains("controller"));
        // A MAC is often the tail of the Host name: `leaf_ultra_lu862-70b3d50c1637`
        // or `Front-Garage-EA1-000FFF1DCCB1`. Take it, and then take it OFF the
        // name — the operator named the thing "Front-Garage-EA1"; the hex is an
        // implementation detail already shown in its own column.
        u.mac = u.name.as_deref().and_then(mac_from_tail);
        if u.mac.is_some() {
            if let Some(n) = u.name.take() {
                let trimmed = strip_mac_tail(&n);
                u.name = (!trimmed.is_empty()).then_some(trimmed);
            }
        }
        return (u.kind.is_some() || u.model.is_some()).then_some(u);
    }

    // The SSDP dialect: only c4:director is ours.
    let st = field(text, "ST")?;
    if !st.contains(DIRECTOR) {
        return None;
    }
    u.is_director = true;
    // `USN: c4:director:Living-EA1-000FFF9163CB`
    if let Some(usn) = field(text, "USN") {
        let ident = usn.rsplit(':').next().unwrap_or(&usn).to_string();
        u.mac = mac_from_tail(&ident);
        let name = strip_mac_tail(&ident);
        if !name.is_empty() {
            u.name = Some(name);
        }
    }
    Some(u)
}

/// `Name: "value"` or `Name: value`, case-insensitive, quotes stripped.
fn field(msg: &str, name: &str) -> Option<String> {
    msg.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if !k.trim().eq_ignore_ascii_case(name) {
            return None;
        }
        let v = v.trim().trim_matches('"').trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Drop a trailing MAC from a name, leaving what a person actually called it.
fn strip_mac_tail(s: &str) -> String {
    match s.rfind(['-', '_']) {
        Some(i) if mac_from_tail(s).is_some() => s[..i].to_string(),
        _ => s.to_string(),
    }
}

/// Pull a 12-hex-digit MAC off the end of an identity string.
fn mac_from_tail(s: &str) -> Option<String> {
    let tail: String = s.rsplit(['-', '_', ':']).next()?.to_string();
    if tail.len() != 12 || !tail.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        tail.as_bytes()
            .chunks(2)
            .map(|p| String::from_utf8_lossy(p).to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured off a live network 2026-09-10. Controllers announce roughly once
    // every 80 minutes, so these fixtures are the only practical way to test the
    // parser.
    const DIRECTOR_NOTIFY: &str = "NOTIFY * HTTP/1.1\r
Host: 239.255.255.250:1900\r
NTS: ssdp:alive\r
ST: c4:director\r
USN: c4:director:Living-EA1-000FFF9163CB\r
CACHE-CONTROL: max-age = 5000, no-cache=\"Ext\"\r
\r
";

    const SDDP_ALIVE: &str = "NOTIFY ALIVE SDDP/1.0\r
From: \"192.168.1.109:1902\"\r
Host: \"leaf_ultra_lu862-70b3d50c1637\"\r
Max-Age: 1800\r
Type: \"c4:leaf_ultra_hdmivswitch_c\"\r
Primary-Proxy: \"avswitch\"\r
Manufacturer: \"Leaf Audio\"\r
Model: \"ULTRA LU862\"\r
Driver: \"leaf_ultra_lu862_ip.c4i\"\r
Config-URL: \"http://192.168.1.109\"\r
\r
";

    #[test]
    fn the_director_usn_carries_a_name_and_a_mac() {
        let u = parse(DIRECTOR_NOTIFY, "192.168.1.172").expect("a director");
        assert_eq!(u.name.as_deref(), Some("Living-EA1"));
        assert_eq!(u.mac.as_deref(), Some("00:0f:ff:91:63:cb"));
        assert!(u.is_director);
    }

    #[test]
    fn an_sddp_record_carries_the_identity_a_list_should_show() {
        let u = parse(SDDP_ALIVE, "192.168.1.109").expect("a device");
        assert_eq!(u.manufacturer.as_deref(), Some("Leaf Audio"));
        assert_eq!(u.model.as_deref(), Some("ULTRA LU862"));
        assert_eq!(u.kind.as_deref(), Some("c4:leaf_ultra_hdmivswitch_c"));
        assert_eq!(u.config_url.as_deref(), Some("http://192.168.1.109"));
        // Not a controller: the type says avswitch.
        assert!(!u.is_director);
        // The MAC is the tail of the Host name, and is removed from the name.
        assert_eq!(u.mac.as_deref(), Some("70:b3:d5:0c:16:37"));
        assert_eq!(u.name.as_deref(), Some("leaf_ultra_lu862"));
    }

    #[test]
    fn a_name_with_a_mac_glued_on_is_shown_without_it() {
        // Both dialects do this and the operator named neither of them that.
        let sddp = SDDP_ALIVE.replace("leaf_ultra_lu862-70b3d50c1637", "Front-Garage-EA1-000FFF1DCCB1");
        let u = parse(&sddp, "192.168.1.139").unwrap();
        assert_eq!(u.name.as_deref(), Some("Front-Garage-EA1"));
        assert_eq!(u.mac.as_deref(), Some("00:0f:ff:1d:cc:b1"));
        // A name that merely contains a hyphen keeps all of it.
        assert_eq!(strip_mac_tail("Living-EA1"), "Living-EA1");
    }

    #[test]
    fn our_own_searches_are_not_discoveries() {
        assert!(parse(&String::from_utf8(ssdp_msearch()).unwrap(), "10.0.0.5").is_none());
        assert!(parse(
            &String::from_utf8(sddp_search("10.0.0.5".parse().unwrap())).unwrap(),
            "10.0.0.5"
        )
        .is_none());
    }

    #[test]
    fn a_device_saying_goodbye_is_not_present() {
        let bye = "NOTIFY OFFLINE SDDP/1.0\r\nHost: \"x\"\r\nFrom: \"192.168.1.112:1902\"\r\nType: \"Denon:AVR-X2100W\"\r\n\r\n";
        assert!(parse(bye, "192.168.1.112").is_none());
    }

    #[test]
    fn non_control4_ssdp_is_ignored() {
        let upnp = "NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nST: upnp:rootdevice\r\nUSN: uuid:bf891288\r\n\r\n";
        assert!(parse(upnp, "192.168.1.1").is_none());
    }

    #[test]
    fn searches_are_well_formed() {
        let m = String::from_utf8(ssdp_msearch()).unwrap();
        assert!(m.starts_with("M-SEARCH * HTTP/1.1") && m.contains("ST: c4:director"));
        assert!(m.ends_with("\r\n\r\n"));
        let s = String::from_utf8(sddp_search("10.0.0.5".parse().unwrap())).unwrap();
        // The Host value has to be the sender's ADDRESS; a hostname here means
        // replies are addressed somewhere the sender is not listening.
        assert!(s.contains("Host: \"10.0.0.5:1902\""));
    }
}
