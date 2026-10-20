//! Live port state, read straight from the kernel — never stored.
//!
//! Everything here comes from `/sys/class/net/<iface>/`, which is what the b53
//! DSA driver populates for each slave (`lan1`, `lan2`): carrier reflects the
//! real PHY link, `speed`/`duplex` the negotiated rate, `statistics/*` the
//! per-port counters the switch reports. Reading sysfs rather than parsing `ip`
//! keeps this allocation-light (it runs on a timer) and avoids depending on the
//! exact `ip -j` JSON shape across busybox/iproute2 versions.
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Default, utoipa::ToSchema)]
pub struct PortStatus {
    pub name: String,
    /// The netdev exists (the DSA slave was created). False means the switch
    /// driver did not bring this port up — the single most useful bit when DSA
    /// bring-up is in doubt.
    pub present: bool,
    /// Link carrier — a cable is plugged in and the PHY is up.
    pub carrier: bool,
    /// `operstate` verbatim (`up`, `down`, `lowerlayerdown`, …).
    pub operstate: String,
    /// Negotiated speed in Mbit/s, when the link is up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<i64>,
    /// `full` / `half`, when the link is up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplex: Option<String>,
    /// The bridge this port is enslaved to right now, if any (its `master`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    /// Admin state (IFF_UP) — distinct from carrier: a port can be admin-up with
    /// no cable.
    pub admin_up: bool,
    pub stats: PortStats,
}

#[derive(Clone, Debug, Serialize, Default, utoipa::ToSchema)]
pub struct PortStats {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

fn sysfs(iface: &str, leaf: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/{leaf}"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn num(iface: &str, leaf: &str) -> u64 {
    sysfs(iface, leaf).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// The bridge a port is enslaved to, from the `master` symlink's target name.
fn master_of(iface: &str) -> Option<String> {
    std::fs::read_link(format!("/sys/class/net/{iface}/master"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// `flags` is a hex bitfield; IFF_UP is bit 0. Admin-up is independent of
/// whether a cable is present (that is `carrier`).
fn admin_up(iface: &str) -> bool {
    sysfs(iface, "flags")
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .map(|f| f & 0x1 != 0)
        .unwrap_or(false)
}

pub fn status(name: &str) -> PortStatus {
    let present = std::path::Path::new(&format!("/sys/class/net/{name}")).exists();
    if !present {
        return PortStatus {
            name: name.to_string(),
            present: false,
            operstate: "absent".into(),
            ..PortStatus::default()
        };
    }
    let carrier = sysfs(name, "carrier").as_deref() == Some("1");
    // speed/duplex are only meaningful (and only readable without -EINVAL) while
    // the link is up; sysfs returns them as -1 / "unknown" otherwise.
    let (speed, duplex) = if carrier {
        (
            sysfs(name, "speed").and_then(|s| s.parse().ok()).filter(|&s: &i64| s > 0),
            sysfs(name, "duplex").filter(|d| d == "full" || d == "half"),
        )
    } else {
        (None, None)
    };
    PortStatus {
        name: name.to_string(),
        present: true,
        carrier,
        operstate: sysfs(name, "operstate").unwrap_or_else(|| "unknown".into()),
        speed,
        duplex,
        master: master_of(name),
        admin_up: admin_up(name),
        stats: PortStats {
            rx_bytes: num(name, "statistics/rx_bytes"),
            tx_bytes: num(name, "statistics/tx_bytes"),
            rx_packets: num(name, "statistics/rx_packets"),
            tx_packets: num(name, "statistics/tx_packets"),
            rx_errors: num(name, "statistics/rx_errors"),
            tx_errors: num(name, "statistics/tx_errors"),
        },
    }
}

/// Does a netdev (a bridge, say) exist right now?
pub fn exists(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}")).exists()
}

/// IPv4 addresses on an interface, as `addr/cidr` strings — for the overview, so
/// the UI can show where an address actually landed.
pub fn addrs(iface: &str) -> Vec<String> {
    // `ip -o -4 addr show dev X` → one line per address; field 4 is addr/cidr.
    let out = std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show", "dev", iface])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.split_whitespace().nth(3).map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}
