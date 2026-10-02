//! Shared Wi-Fi setup helpers, used by two separate binaries: the captive-portal
//! app (portal, serves the setup page while the AP is up) and the dashboard
//! (webd, exposes the same control over its API). Neither does wireless I/O
//! directly — the shared S41wifi-ap script drops scanned SSIDs into
//! /tmp/wifi-scan, and joining here writes a wpa_supplicant station config
//! then kicks that script to switch wlan from AP to station.
const SCAN_CACHE: &str = "/tmp/wifi-scan";
const BOARD_ENV: &str = "/opt/ohc/board.env";

/// The board's Wi-Fi interface (OHC_WIFI_IFACE in board.env), empty if none.
pub fn wifi_iface() -> String {
    std::fs::read_to_string(BOARD_ENV)
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.trim().strip_prefix("OHC_WIFI_IFACE="))
        .map(|v| v.split('#').next().unwrap_or("").trim().trim_matches('"').trim_matches('\'').to_string())
        .unwrap_or_default()
}

/// SSIDs captured by S41wifi-ap at AP start (empty if the scan found nothing).
pub fn scan_cache() -> Vec<String> {
    std::fs::read_to_string(SCAN_CACHE)
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// wpa_supplicant quoted-string escaping, dropping control chars so a crafted
/// SSID/password can't inject extra config lines.
fn wpa_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| !c.is_control()) {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// Write the station config for `iface` and kick S41wifi-ap to join. Returns the
/// SSID on success. The AP is torn down by that restart (after this response
/// flushes), so the client that submitted this necessarily loses the connection.
pub fn apply(iface: &str, ssid: &str, psk: &str) -> Result<String, String> {
    if ssid.is_empty() {
        return Err("ssid required".into());
    }
    if iface.is_empty() {
        return Err("this board has no wifi radio".into());
    }
    let net = if psk.is_empty() {
        format!("network={{\n\tssid=\"{}\"\n\tkey_mgmt=NONE\n}}\n", wpa_str(ssid))
    } else {
        format!("network={{\n\tssid=\"{}\"\n\tpsk=\"{}\"\n}}\n", wpa_str(ssid), wpa_str(psk))
    };
    let conf = format!("ctrl_interface=/var/run/wpa_supplicant\nupdate_config=1\n{net}");
    std::fs::create_dir_all("/etc/wpa_supplicant").ok();
    let path = format!("/etc/wpa_supplicant/wpa_supplicant-{iface}.conf");
    std::fs::write(&path, conf).map_err(|e| e.to_string())?;
    // Delay so this HTTP response reaches the phone before the AP drops.
    std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 2; /etc/init.d/S41wifi-ap restart >/dev/null 2>&1")
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(ssid.to_string())
}

/// Self-contained setup page (no React, no assets) — served for every path while
/// the AP is up, which is what trips the OS captive-portal check. Authored as a
/// real HTML file in assets/ and embedded at compile time, so it edits like a web
/// page but still ships inside the single static binary (no runtime file to find).
pub const PORTAL_HTML: &str = include_str!("../assets/portal.html");
