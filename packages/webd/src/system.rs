//! Runtime system state — read straight from /proc and /sys (no shelling out for
//! the basics; `ip` is used only to format addresses).
use serde::Serialize;

#[derive(Serialize)]
pub struct Iface {
    pub name: String,
    pub ip: String,
    pub carrier: bool,
}

#[derive(Serialize)]
pub struct System {
    pub hostname: String,
    pub kernel: String,
    pub uptime: f64,
    pub loadavg: String,
    pub interfaces: Vec<Iface>,
}

fn rd(p: &str) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn ipv4(iface: &str) -> String {
    // parse `ip -4 -o addr show <iface>`; cheap and avoids a netlink dep.
    if let Ok(out) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", iface])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(i) = s.find("inet ") {
            return s[i + 5..].split_whitespace().next().unwrap_or("").to_string();
        }
    }
    String::new()
}

pub fn snapshot() -> System {
    let mut interfaces = Vec::new();
    if let Ok(rd_dir) = std::fs::read_dir("/sys/class/net") {
        let mut names: Vec<String> = rd_dir
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "lo")
            .collect();
        names.sort();
        for n in names {
            interfaces.push(Iface {
                ip: ipv4(&n),
                carrier: rd(&format!("/sys/class/net/{n}/carrier")) == "1",
                name: n,
            });
        }
    }
    System {
        hostname: crate::board::hostname(),
        kernel: rd("/proc/sys/kernel/osrelease"),
        uptime: rd("/proc/uptime").split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
        loadavg: rd("/proc/loadavg"),
        interfaces,
    }
}
