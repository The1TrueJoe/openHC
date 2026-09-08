//! The board capability model, parsed from /opt/ohc/board.env — the same shell
//! env the init scripts source. This is what makes the UI board-agnostic: a CA-1
//! (zwave + zigbee + one combo serial + wifi) and a 3-serial/zigbee-only board
//! differ only in this file.
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct Radio {
    #[serde(rename = "type")]
    pub kind: String,
    pub dev: String,
}

#[derive(Serialize, Clone)]
pub struct SerialPort {
    pub dev: String,
    pub label: String,
    pub baud: u32,
}

#[derive(Serialize, Clone)]
pub struct Board {
    pub model: String,
    pub hostname: String,
    pub uplink_iface: String,
    pub wifi_iface: String,
    pub radios: Vec<Radio>,
    pub serials: Vec<SerialPort>,
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
        let radios = e
            .get("OHC_RADIOS")
            .map(|s| {
                s.split_whitespace()
                    .filter_map(|t| t.split_once(':').map(|(k, d)| Radio { kind: k.into(), dev: d.into() }))
                    .collect()
            })
            .unwrap_or_default();
        let serials = e
            .get("OHC_SERIALS")
            .map(|s| {
                s.split_whitespace()
                    .map(|t| {
                        let p: Vec<&str> = t.split(':').collect();
                        SerialPort {
                            dev: format!("/dev/{}", p.first().unwrap_or(&"")),
                            label: p.get(1).map(|x| x.replace('_', " ")).unwrap_or_default(),
                            baud: p.get(2).and_then(|x| x.parse().ok()).unwrap_or(115200),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Board {
            model: e.get("OHC_MODEL").cloned().unwrap_or_else(|| "unknown".into()),
            hostname: hostname(),
            uplink_iface: e.get("OHC_UPLINK_IFACE").cloned().unwrap_or_else(|| "eth0".into()),
            wifi_iface: e.get("OHC_WIFI_IFACE").cloned().unwrap_or_default(),
            radios,
            serials,
        }
    }

    pub fn radio_dev(&self, kind: &str) -> Option<String> {
        self.radios.iter().find(|r| r.kind == kind).map(|r| r.dev.clone())
    }

    pub fn serial_baud(&self, dev: &str) -> u32 {
        self.serials.iter().find(|s| s.dev == dev).map(|s| s.baud).unwrap_or(115200)
    }
}
