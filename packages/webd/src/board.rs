//! Board IDENTITY and NETWORK, parsed from /opt/ohc/board.env.
//!
//! Deliberately does NOT model radios, serial ports or any other IO. Those moved
//! to iod when it took ownership of the devices — webd describing ports it can
//! no longer open would be a second, drifting source of truth. A client asks iod
//! `GET /api/io` for that, and webd for what it still owns: which box this is,
//! and how it is on the network.
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct Board {
    pub model: String,
    pub hostname: String,
    pub uplink_iface: String,
    pub wifi_iface: String,
}

fn read_env(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(path) {
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                continue;
            }
            let (k, v) = line.split_once('=').unwrap();
            let v = v.split('#').next().unwrap_or("").trim().trim_matches('"').trim_matches('\'');
            m.insert(k.trim().to_string(), v.to_string());
        }
    }
    m
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "openhc".into())
}

impl Board {
    pub fn load(env_path: &str) -> Board {
        let e = read_env(env_path);
        Board {
            model: e.get("OHC_MODEL").cloned().unwrap_or_else(|| "unknown".into()),
            hostname: hostname(),
            uplink_iface: e.get("OHC_UPLINK_IFACE").cloned().unwrap_or_else(|| "eth0".into()),
            wifi_iface: e.get("OHC_WIFI_IFACE").cloned().unwrap_or_default(),
        }
    }


}
