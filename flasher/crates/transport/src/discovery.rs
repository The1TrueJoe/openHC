//! Find Control4 boxes on the network without being told where they are.
//!
//! A freshly restored unit answers no friendly protocol, so the load-bearing
//! signal is its MAC: Control4 owns OUI 00:0f:ff, and an ARP table (optionally
//! after a ping sweep) surfaces units by hardware address regardless of
//! hostname, DHCP or mDNS state.
//!
//! EVERY SHELL-OUT IN HERE IS PLATFORM SPECIFIC, and getting that wrong is not a
//! degraded scan, it is a silent empty one. This module found nothing at all on
//! Windows for exactly that reason: `ifconfig` does not exist there, so no
//! subnet was ever derived; `ping -c 1` is a usage error, so the sweep pinged
//! nothing; and `arp -a` prints MACs as `00-0f-ff-...`, which no colon-splitting
//! parser will ever match. Three independent failures, one symptom — an empty
//! list that looks exactly like a quiet network.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::process::Command;
use std::time::Duration;
use std::sync::mpsc;
use std::thread;

/// Control4's OUI.
const C4_OUI: [&str; 2] = ["00:0f:ff", "0:f:ff"];

#[derive(Debug, Clone, Default)]
pub struct Found {
    pub ip: String,
    pub mac: Option<String>,
    pub via: Vec<String>,
    /// Identity from SDDP, when the unit announced any. See `sddp` for what the
    /// protocol does and does not carry — notably it carries no firmware
    /// version, on any unit measured here.
    pub name: Option<String>,
    pub kind: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub config_url: Option<String>,
    /// It answered as a Control4 *controller*, not merely a device.
    pub is_director: bool,
}

impl Found {
    /// Is this Control4 HARDWARE, as opposed to something Control4 merely
    /// drives?
    ///
    /// The SDDP bus carries the whole system — televisions, amplifiers, DVRs,
    /// power strips — and a `c4:` type prefix does not make a device Control4's:
    /// `c4:leaf_ultra_hdmivswitch_c` is a Leaf Audio switch, named for the
    /// driver that speaks to it. The OUI is the signal that does not lie.
    pub fn is_control4(&self) -> bool {
        self.is_director
            || self
                .mac
                .as_deref()
                .is_some_and(|m| C4_OUI.iter().any(|o| m.starts_with(o)))
    }
}

/// Normalise a MAC to lower-case colon form.
///
/// Accepts colons (Unix `arp`), dashes (Windows `arp -a`) and dotted Cisco
/// notation, because the three platforms we run on do not agree and the
/// difference is invisible until a lookup silently matches nothing.
fn norm_mac(m: &str) -> String {
    m.split(['-', ':', '.'])
        .filter_map(|p| u8::from_str_radix(p, 16).ok())
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Does this token look like a MAC in any of the notations above?
fn looks_like_mac(w: &str) -> bool {
    let parts: Vec<&str> = w.split(['-', ':']).collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() <= 2 && u8::from_str_radix(p, 16).is_ok())
}

/// `ping` for one host, spelled the way this platform spells it.
///
/// Windows: `-n <count> -w <milliseconds>`.
/// macOS:   `-c <count> -W <milliseconds>`.
/// Linux:   `-c <count> -W <SECONDS>` — note the unit changes, so passing the
///          millisecond value there would ask for a 300-second timeout.
fn ping_cmd(ip: &str, millis: u32) -> Command {
    let mut c = Command::new("ping");
    if cfg!(windows) {
        c.args(["-n", "1", "-w", &millis.to_string(), ip]);
    } else if cfg!(target_os = "macos") {
        c.args(["-c", "1", "-W", &millis.to_string(), ip]);
    } else {
        let secs = millis.div_ceil(1000).max(1);
        c.args(["-c", "1", "-W", &secs.to_string(), ip]);
    }
    c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c
}

/// Parse the OS ARP/neighbour table into ip -> mac.
pub fn arp_table() -> BTreeMap<String, String> {
    let mut table = BTreeMap::new();
    // `arp -a` is the one spelling all three platforms accept: Unix treats the
    // missing `-n` as "resolve names", which is slower but still prints the
    // addresses, and Windows rejects `-an` outright.
    for argv in [&["arp", "-a"][..], &["ip", "neigh"][..]] {
        let Ok(out) = Command::new(argv[0]).args(&argv[1..]).output() else { continue };
        table.extend(parse_arp(&String::from_utf8_lossy(&out.stdout)));
        if !table.is_empty() {
            break;
        }
    }
    table
}

/// Ping every host on the given /24s so the neighbour table populates. We do
/// not care which pings answer — only which MACs appear — because a stock unit
/// may drop ICMP but still ARP.
pub fn sweep(subnets: &[[u8; 3]]) {
    let (tx, rx) = mpsc::channel::<()>();
    let mut handles = vec![];
    // Bounded fan-out so we do not spawn 254*n threads at once.
    let jobs: Vec<String> = subnets
        .iter()
        .flat_map(|net| (1u16..=254).map(move |h| format!("{}.{}.{}.{}", net[0], net[1], net[2], h)))
        .collect();
    let chunk = jobs.len().div_ceil(64).max(1);
    for group in jobs.chunks(chunk) {
        let group: Vec<String> = group.to_vec();
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            for ip in group {
                let _ = ping_cmd(&ip, 300).output();
            }
            let _ = tx.send(());
        }));
    }
    drop(tx);
    while rx.recv().is_ok() {}
    for h in handles {
        let _ = h.join();
    }
}

/// The /24s this host is on, so the sweep stays bounded.
pub fn local_subnets() -> Vec<[u8; 3]> {
    let mut nets = vec![];
    for ip in local_v4() {
        let o = ip.octets();
        if o[0] != 127 {
            let net = [o[0], o[1], o[2]];
            if !nets.contains(&net) {
                nets.push(net);
            }
        }
    }
    nets
}

/// Every local IPv4 address, asked for in whichever dialect this OS speaks.
///
/// Tried in order until one produces something: a machine with `ip` but no
/// `ifconfig` (modern Linux) and one with `ifconfig` but no `ip` (macOS) both
/// have to work, and Windows has neither.
///
/// This returns ALL of them on purpose. A laptop is very often on two networks
/// at once — this one answers on four — and the controller is reachable on
/// exactly one, so picking a single interface is a coin flip.
pub fn local_v4() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = vec![];
    fn keep(out: &mut Vec<Ipv4Addr>, ip: Ipv4Addr) {
        if !ip.is_loopback() && !ip.is_unspecified() && !out.contains(&ip) {
            out.push(ip);
        }
    }

    for argv in [
        &["ifconfig"][..],          // macOS, BSD
        &["ip", "-4", "-o", "addr"][..], // Linux
        &["ipconfig"][..],          // Windows
    ] {
        let Ok(o) = Command::new(argv[0]).args(&argv[1..]).output() else { continue };
        for ip in parse_v4(&String::from_utf8_lossy(&o.stdout)) {
            keep(&mut out, ip);
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// ip -> mac out of an ARP/neighbour table, in any of the three formats.
///
/// Pure, because the only way to be sure the Windows branch works is to feed it
/// real Windows output — and that cannot be produced on the machine this is
/// developed on.
pub fn parse_arp(text: &str) -> BTreeMap<String, String> {
    let mut table = BTreeMap::new();
    for line in text.lines() {
        let ip = line
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .find(|s| s.split('.').count() == 4 && !s.is_empty());
        let mac = line.split_whitespace().find(|w| looks_like_mac(w));
        let (Some(ip), Some(mac)) = (ip, mac) else { continue };
        // Windows prints the multicast and broadcast rows too; they are not
        // hosts and their "MACs" would pollute an OUI match.
        let n = norm_mac(mac);
        if n == "ff:ff:ff:ff:ff:ff" || n == "00:00:00:00:00:00" || n.starts_with("01:00:5e") {
            continue;
        }
        // ...and Windows' header line ("Interface: 192.168.1.146 --- 0xa") has an
        // address but no MAC, so it never reaches here.
        table.insert(ip.to_string(), n);
    }
    table
}

/// Every IPv4 address in `ifconfig` / `ip -o addr` / `ipconfig` output.
pub fn parse_v4(text: &str) -> Vec<Ipv4Addr> {
    let mut out = vec![];
    for line in text.lines() {
        let t = line.trim();
        // `inet 10.0.0.5 netmask ...`         (ifconfig)
        // `2: eth0 inet 10.0.0.5/24 ...`      (ip -o)
        // `IPv4 Address. . . . . . : 10.0.0.5` (ipconfig — the dots are padding,
        //                                       and the label is localised on a
        //                                       non-English install, so the match
        //                                       is deliberately loose)
        let cand = if let Some(r) = t.strip_prefix("inet ") {
            r.split_whitespace().next()
        } else if t.contains(" inet ") {
            t.split(" inet ").nth(1).and_then(|r| r.split_whitespace().next())
        } else if t.contains("IPv4") || t.contains("IP Address") {
            t.rsplit(':').next().map(str::trim)
        } else {
            None
        };
        if let Some(c) = cand {
            if let Ok(ip) = c.split('/').next().unwrap_or(c).trim().parse::<Ipv4Addr>() {
                out.push(ip);
            }
        }
    }
    out
}

/// Does this address still answer? One ping, short timeout. See `ping_cmd` for
/// why that is three different command lines.
fn alive(ip: &str) -> bool {
    ping_cmd(ip, 1000).status().map(|s| s.success()).unwrap_or(false)
}

/// Every Control4 unit we can find: SDDP responders (positively identified) and
/// ARP entries whose MAC is in Control4's OUI, merged by IP.
///
/// SDDP is the stronger signal — a box that answers it *is* a controller, even
/// one too freshly restored to have a hostname or an ARP entry — so an
/// SDDP-only responder is kept even when its MAC is not the Control4 OUI (a unit
/// behind a third-party NIC chip). ARP then fills in a MAC SDDP did not carry.
/// Control4 hardware only — what a front end should show by default.
///
/// Both front ends need the same answer to "is this a controller", and when the
/// CLI had this rule and the GUI did not, the same scan listed a Samsung TV in
/// one and not the other.
pub fn discover(do_sweep: bool) -> Vec<Found> {
    let mut v = discover_all(do_sweep);
    v.retain(|f| f.is_control4());
    v
}

/// Everything that answered, including the non-Control4 devices on the bus.
pub fn discover_all(do_sweep: bool) -> Vec<Found> {
    if do_sweep {
        sweep(&local_subnets());
    }
    let mut by_ip: BTreeMap<String, Found> = BTreeMap::new();
    let arp = arp_table();

    // Ask the Control4-looking addresses DIRECTLY as well as on the multicast
    // group. Multicast leaves one interface and these units are routinely on a
    // different subnet from the default route; a unicast search reaches them.
    let candidates: Vec<Ipv4Addr> = arp
        .iter()
        .filter(|(_, mac)| C4_OUI.iter().any(|o| mac.starts_with(o)))
        .filter_map(|(ip, _)| ip.parse().ok())
        .collect();

    for u in crate::sddp::search_from(&candidates, Duration::from_millis(1800)) {
        by_ip.insert(
            u.ip.clone(),
            Found {
                ip: u.ip,
                mac: u.mac,
                via: vec!["sddp".into()],
                name: u.name,
                kind: u.kind,
                manufacturer: u.manufacturer,
                model: u.model,
                config_url: u.config_url,
                is_director: u.is_director,
            },
        );
    }

    for (ip, mac) in arp {
        let is_c4 = C4_OUI.iter().any(|o| mac.starts_with(o));
        match by_ip.get_mut(&ip) {
            // SDDP already found it — record the MAC it did not carry.
            Some(f) => {
                f.mac.get_or_insert(mac);
                f.via.push("arp".into());
            }
            // Not seen by SDDP: include it only if the MAC says Control4, AND
            // only if it still answers.
            //
            // THE ARP TABLE REMEMBERS ADDRESSES A UNIT NO LONGER HAS. These
            // controllers take a DHCP lease on every boot and move — an HC-800
            // was observed on both .111 and .112 inside an hour — and the old
            // entry survives for minutes afterwards. Listing it unchecked hands
            // the operator a dead address that looks exactly like a live one,
            // which is the single most expensive mistake available here: it
            // reads as "the box did not come back" and has cost real power
            // cycles. SDDP responders are exempt because answering SDDP is
            // itself proof of life.
            None if is_c4 && alive(&ip) => {
                by_ip.insert(
                    ip.clone(),
                    Found { ip, mac: Some(mac), via: vec!["arp".into()], ..Default::default() },
                );
            }
            None => {}
        }
    }

    by_ip.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real output from all three platforms. These fixtures are the entire
    // reason the parsers above are pure: the Windows branch cannot be exercised
    // on the machine this is written on, and "it compiles" is not evidence that
    // it parses.

    const WINDOWS_ARP: &str = "\r
Interface: 192.168.1.146 --- 0xa\r
  Internet Address      Physical Address      Type\r
  192.168.1.1           00-1a-2b-3c-4d-5e     dynamic\r
  192.168.1.172         00-0f-ff-91-63-cb     dynamic\r
  239.255.255.250       01-00-5e-7f-ff-fa     static\r
  255.255.255.255       ff-ff-ff-ff-ff-ff     static\r
";

    const MACOS_ARP: &str = "\
? (10.0.0.112) at 0:f:ff:57:b9:78 on en0 ifscope [ethernet]
? (10.0.0.1) at 1c:limit:bad on en0 [ethernet]
? (192.168.1.172) at 0:f:ff:91:63:cb on en1 ifscope [ethernet]
";

    const LINUX_NEIGH: &str = "\
10.0.0.112 dev eth0 lladdr 00:0f:ff:57:b9:78 REACHABLE
10.0.0.1 dev eth0 lladdr 1c:3b:f3:aa:bb:cc STALE
";

    #[test]
    fn windows_arp_uses_dashes_and_lists_rows_that_are_not_hosts() {
        let t = parse_arp(WINDOWS_ARP);
        // The dashed MAC has to survive, or Windows finds nothing at all.
        assert_eq!(t.get("192.168.1.172").map(String::as_str), Some("00:0f:ff:91:63:cb"));
        assert_eq!(t.get("192.168.1.1").map(String::as_str), Some("00:1a:2b:3c:4d:5e"));
        // Broadcast and multicast rows are not units.
        assert!(!t.contains_key("255.255.255.255"));
        assert!(!t.contains_key("239.255.255.250"));
        // The "Interface:" header has an address but no MAC.
        assert!(!t.contains_key("192.168.1.146"));
    }

    #[test]
    fn macos_arp_zero_pads_and_linux_neigh_parses() {
        let t = parse_arp(MACOS_ARP);
        // macOS prints `0:f:ff:...`; a Control4 OUI match needs `00:0f:ff:...`.
        assert_eq!(t.get("10.0.0.112").map(String::as_str), Some("00:0f:ff:57:b9:78"));
        assert!(C4_OUI.iter().any(|o| t["10.0.0.112"].starts_with(o)));

        let t = parse_arp(LINUX_NEIGH);
        assert_eq!(t.get("10.0.0.112").map(String::as_str), Some("00:0f:ff:57:b9:78"));
    }

    #[test]
    fn ipconfig_is_the_only_way_to_get_a_subnet_on_windows() {
        let out = parse_v4(
            "\r
Windows IP Configuration\r
\r
Ethernet adapter Ethernet:\r
\r
   Connection-specific DNS Suffix  . : \r
   Link-local IPv6 Address . . . . . : fe80::1234:5678:9abc:def0%10\r
   IPv4 Address. . . . . . . . . . . : 192.168.1.146\r
   Subnet Mask . . . . . . . . . . . : 255.255.255.0\r
   Default Gateway . . . . . . . . . : 192.168.1.1\r
",
        );
        // The address, not the mask and not the gateway: a subnet derived from
        // 255.255.255.0 would sweep 255.255.255.0/24 and find nothing, which is
        // indistinguishable from a quiet network.
        assert!(out.contains(&"192.168.1.146".parse().unwrap()));
        assert!(!out.contains(&"255.255.255.0".parse().unwrap()));
    }

    #[test]
    fn ifconfig_and_ip_addr_both_parse() {
        let mac = parse_v4("\
en0: flags=8863<UP,BROADCAST> mtu 1500
\tether 4c:20:b8:a7:ea:59
\tinet 10.0.0.105 netmask 0xffffff00 broadcast 10.0.0.255
\tinet6 fe80::1%en0 prefixlen 64
");
        assert_eq!(mac, vec!["10.0.0.105".parse::<Ipv4Addr>().unwrap()]);

        let lin = parse_v4("2: eth0    inet 10.0.0.105/24 brd 10.0.0.255 scope global eth0\\       valid_lft forever");
        assert_eq!(lin, vec!["10.0.0.105".parse::<Ipv4Addr>().unwrap()]);
    }

    #[test]
    fn ping_is_spelled_differently_on_every_platform() {
        // Not asserting the platform we happen to be on — asserting that the
        // flags are never the Unix ones on Windows, because `ping -c 1` there is
        // a usage error that makes every probe silently fail.
        let c = ping_cmd("10.0.0.1", 300);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        if cfg!(windows) {
            assert!(args.contains(&"-n".into()) && args.contains(&"-w".into()));
            assert!(!args.contains(&"-c".into()));
        } else {
            assert!(args.contains(&"-c".into()) && args.contains(&"-W".into()));
        }
        assert!(args.contains(&"10.0.0.1".into()));
    }
}
