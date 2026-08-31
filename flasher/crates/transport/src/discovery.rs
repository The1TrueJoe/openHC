//! Find Control4 boxes on the network without being told where they are.
//!
//! A freshly restored unit answers no friendly protocol, so the load-bearing
//! signal is its MAC: Control4 owns OUI 00:0f:ff, and an ARP table (optionally
//! after a ping sweep) surfaces units by hardware address regardless of
//! hostname, DHCP or mDNS state.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::process::Command;
use std::time::Duration;
use std::sync::mpsc;
use std::thread;

/// Control4's OUI.
const C4_OUI: [&str; 2] = ["00:0f:ff", "0:f:ff"];

#[derive(Debug, Clone)]
pub struct Found {
    pub ip: String,
    pub mac: Option<String>,
    pub via: Vec<String>,
}

fn norm_mac(m: &str) -> String {
    m.split(':')
        .filter_map(|p| u8::from_str_radix(p, 16).ok())
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Parse the OS ARP/neighbour table into ip -> mac.
pub fn arp_table() -> BTreeMap<String, String> {
    let mut table = BTreeMap::new();
    for argv in [["arp", "-an"], ["ip", "neigh"]] {
        let Ok(out) = Command::new(argv[0]).arg(argv[1]).output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let ip = line
                .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                .find(|s| s.split('.').count() == 4 && !s.is_empty());
            let mac = line
                .split_whitespace()
                .find(|w| w.split(':').count() == 6 && w.contains(':'));
            if let (Some(ip), Some(mac)) = (ip, mac) {
                table.insert(ip.to_string(), norm_mac(mac));
            }
        }
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
                let _ = Command::new("ping")
                    .args(["-c", "1", "-W", "300", &ip])
                    .output();
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
    let out = Command::new("ifconfig").output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ipstr) = rest.split_whitespace().next() {
                if let Ok(ip) = ipstr.parse::<Ipv4Addr>() {
                    let o = ip.octets();
                    if o[0] != 127 {
                        let net = [o[0], o[1], o[2]];
                        if !nets.contains(&net) {
                            nets.push(net);
                        }
                    }
                }
            }
        }
    }
    nets
}

/// Every Control4 unit we can find: SDDP responders (positively identified) and
/// ARP entries whose MAC is in Control4's OUI, merged by IP.
///
/// SDDP is the stronger signal — a box that answers it *is* a controller, even
/// one too freshly restored to have a hostname or an ARP entry — so an
/// SDDP-only responder is kept even when its MAC is not the Control4 OUI (a unit
/// behind a third-party NIC chip). ARP then fills in a MAC SDDP did not carry.
pub fn discover(do_sweep: bool) -> Vec<Found> {
    if do_sweep {
        sweep(&local_subnets());
    }
    let mut by_ip: BTreeMap<String, Found> = BTreeMap::new();

    for u in crate::sddp::search(Duration::from_millis(1500)) {
        by_ip.insert(
            u.ip.clone(),
            Found { ip: u.ip, mac: u.mac, via: vec!["sddp".into()] },
        );
    }

    for (ip, mac) in arp_table() {
        let is_c4 = C4_OUI.iter().any(|o| mac.starts_with(o));
        match by_ip.get_mut(&ip) {
            // SDDP already found it — record the MAC it did not carry.
            Some(f) => {
                f.mac.get_or_insert(mac);
                f.via.push("arp".into());
            }
            // Not seen by SDDP: include it only if the MAC says Control4.
            None if is_c4 => {
                by_ip.insert(ip.clone(), Found { ip, mac: Some(mac), via: vec!["arp".into()] });
            }
            None => {}
        }
    }

    by_ip.into_values().collect()
}
