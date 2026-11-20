//! USB/IP export (the 'usbip' feature). OFF by default.
//!
//! The setting is /data/ohc/usbip.conf, sourced by S97ohcusbip; POST writes it
//! and runs the script's `reload`, which brings usbipd and the usbip-host bindings
//! in line with it. Devices come straight from /sys/bus/usb/devices.
//!
//! The box's own radios are never offered: busids in board.env
//! OHC_USBIP_RESERVED, plus every USB device a radio tty hangs off — the ttys
//! the board's udev rule names /dev/ohc/radio/<kind> (the EA's CP2104 Zigbee
//! bridge, wherever it enumerates). The init script applies the same rule.
use crate::{api::err, Config};
use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use std::{collections::HashSet, path::{Path, PathBuf}, sync::Arc};

const SVC: &str = "/etc/init.d/S97ohcusbip";
const USB: &str = "/sys/bus/usb/devices";
const STUB: &str = "usbip-host";

fn conf_path() -> PathBuf {
    let data = std::env::var("OHC_DATA").unwrap_or_else(|_| "/data".into());
    Path::new(&data).join("ohc/usbip.conf")
}

/// `1-1`, `2-1.4.3`: a USB device (not a root hub `usb1`, not an interface `1-1:1.0`).
fn is_busid(s: &str) -> bool {
    let Some((bus, port)) = s.split_once('-') else { return false };
    !bus.is_empty() && bus.bytes().all(|b| b.is_ascii_digit())
        && !port.is_empty() && !port.starts_with('.') && !port.ends_with('.') && !port.contains("..")
        && port.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Every USB device on a sysfs path — a tty's device link resolves through the
/// device it belongs to (…/usb1/1-1/1-1:1.0/ttyUSB0 → 1-1).
fn busids_in(path: &Path) -> impl Iterator<Item = String> + '_ {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|c| is_busid(c))
        .map(String::from)
}

fn reserved(cfg: &Config) -> HashSet<String> {
    let env = crate::board::read_env(&cfg.board_env);
    let mut r: HashSet<String> = env.get("OHC_USBIP_RESERVED").map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    for link in std::fs::read_dir("/dev/ohc/radio").into_iter().flatten().flatten() {
        let Ok(tty) = std::fs::canonicalize(link.path()) else { continue };
        let Some(name) = tty.file_name().and_then(|n| n.to_str()) else { continue };
        if let Ok(p) = std::fs::canonicalize(format!("/sys/class/tty/{name}/device")) {
            r.extend(busids_in(&p));
        }
    }
    r
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct Device {
    busid: String,
    /// vid:pid, e.g. "10c4:ea60".
    id: String,
    product: String,
    manufacturer: String,
    /// Bound to usbip-host right now (exported, or attached by a client).
    exported: bool,
}

fn devices(skip: &HashSet<String>) -> Vec<Device> {
    let rd = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut v: Vec<Device> = std::fs::read_dir(USB).into_iter().flatten().flatten()
        .filter_map(|e| {
            let busid = e.file_name().into_string().ok()?;
            let d = e.path();
            // Hubs cannot be exported, and the radios must not be.
            if !is_busid(&busid) || skip.contains(&busid) || rd(&d, "bDeviceClass") == "09" {
                return None;
            }
            let driver = std::fs::read_link(d.join("driver")).ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            Some(Device {
                id: format!("{}:{}", rd(&d, "idVendor"), rd(&d, "idProduct")),
                product: rd(&d, "product"),
                manufacturer: rd(&d, "manufacturer"),
                exported: driver.as_deref() == Some(STUB),
                busid,
            })
        })
        .collect();
    v.sort_by(|a, b| a.busid.cmp(&b.busid));
    v
}

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema, Default)]
pub struct UsbIp {
    /// Run usbipd (TCP 3240). Off by default.
    enabled: bool,
    /// Busids to export. May name a device that is unplugged right now.
    #[serde(default)]
    selected: Vec<String>,
    /// Read-only: the box's local USB devices that can be shared (hubs and the
    /// box's own radios left out). Ignored on POST.
    #[serde(default, skip_deserializing)]
    devices: Vec<Device>,
    /// Read-only: this board has the 'usbip' feature. Ignored on POST.
    #[serde(default, skip_deserializing)]
    available: bool,
}

fn read_conf() -> UsbIp {
    let mut u = UsbIp::default();
    for line in std::fs::read_to_string(conf_path()).unwrap_or_default().lines() {
        let Some((k, v)) = line.trim().split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "OHC_USBIP_ENABLE" => u.enabled = v == "1",
            "OHC_USBIP_DEVICES" => u.selected = v.split_whitespace().filter(|b| is_busid(b)).map(String::from).collect(),
            _ => {}
        }
    }
    u
}

fn available() -> bool {
    Path::new(SVC).exists() && Path::new("/usr/sbin/usbipd").exists()
}

#[utoipa::path(get, path = "/api/usbip", tag = "USB/IP",
    summary = "USB/IP state and the shareable USB devices",
    description = "Off by default. `devices` lists this box's USB devices by busid — the box's own \
radio bridges and hubs are never listed. A client attaches one with `usbip attach -r <box> -b <busid>`.",
    responses((status = 200, description = "usbip", body = UsbIp)))]
pub async fn usbip_get(State(c): State<Arc<Config>>) -> Json<UsbIp> {
    Json(UsbIp { devices: devices(&reserved(&c)), available: available(), ..read_conf() })
}

#[utoipa::path(post, path = "/api/usbip", tag = "USB/IP",
    summary = "Turn USB/IP on/off and choose the devices to export",
    description = "Writes /data/ohc/usbip.conf and applies it now (S97ohcusbip reload): usbipd \
started or stopped, the selected devices bound to usbip-host and the rest returned to their own \
drivers. The box's own radios are refused. Persists across reboots.",
    request_body = UsbIp,
    responses((status = 200, description = "applied", body = UsbIp), (status = 400, description = "bad busid")))]
pub async fn usbip_set(State(c): State<Arc<Config>>, Json(u): Json<UsbIp>) -> Response {
    if !available() {
        return err(StatusCode::NOT_FOUND, "USB/IP is not part of this board's image");
    }
    let skip = reserved(&c);
    let mut sel: Vec<String> = Vec::new();
    for b in u.selected.iter().map(|b| b.trim()) {
        if !is_busid(b) {
            return err(StatusCode::BAD_REQUEST, &format!("not a USB busid: {b:?}"));
        }
        if skip.contains(b) {
            return err(StatusCode::BAD_REQUEST, &format!("{b} is this controller's own radio and cannot be shared"));
        }
        if !sel.iter().any(|s| s == b) {
            sel.push(b.to_string());
        }
    }
    let path = conf_path();
    let body = format!(
        "# written by webd POST /api/usbip — read by S97ohcusbip\nOHC_USBIP_ENABLE={}\nOHC_USBIP_DEVICES=\"{}\"\n",
        u.enabled as u8, sel.join(" "));
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&path, body) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("cannot write {}: {e}", path.display()));
    }
    // Synchronous: a reload is a modprobe, usbipd -D and a bind per device.
    match std::process::Command::new(SVC).arg("reload").output() {
        Ok(o) if o.status.success() => usbip_get(State(c)).await.into_response(),
        Ok(o) => err(StatusCode::INTERNAL_SERVER_ERROR,
            &format!("saved, but applying failed: {}", String::from_utf8_lossy(&o.stdout).trim())),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("saved, but {SVC}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busids() {
        for ok in ["1-1", "2-1.4", "10-1.2.3"] {
            assert!(is_busid(ok), "{ok}");
        }
        for bad in ["usb1", "1-1:1.0", "1-", "-1", "1-1.", "1-.1", "1-1..2", "1-1 2-1", "a-1", "1-1;reboot"] {
            assert!(!is_busid(bad), "{bad}");
        }
        let p = Path::new("/sys/devices/pci0000:00/0000:00:1d.7/usb1/1-1/1-1.3/1-1.3:1.0/ttyUSB0");
        assert_eq!(busids_in(p).collect::<Vec<_>>(), ["1-1", "1-1.3"]);
    }
}
