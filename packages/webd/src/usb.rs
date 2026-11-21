//! What is plugged into the box's USB ports, and what the box does with each:
//! a drive storaged shares, the box's own radio, a serial adapter, a network
//! interface, a Bluetooth adapter. The web UI's USB page draws from this; USB/IP
//! sharing (usbip.rs) is the same devices seen from the other side.
//!
//! Everything comes from sysfs: a USB device's directory holds its interfaces,
//! and under those whatever the kernel made of it (block/sda, tty/ttyUSB0,
//! net/usb0, bluetooth/hci0, sound/card1, input/input3).
use crate::{usbip, Config};
use axum::{extract::State, Json};
use std::{collections::HashMap, path::Path, sync::Arc};

const USB: &str = "/sys/bus/usb/devices";

/// What the box does with a device (one device can be several: a stick that is
/// both a serial port and the box's Z-Wave radio).
#[derive(serde::Serialize, utoipa::ToSchema, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Use {
    /// A disk: storaged mounts it and shares it.
    Storage { block: String },
    /// One of the box's radios (`zigbee`, `zwave`), by the board's udev name.
    Radio { radio: String, tty: String },
    /// A serial port nothing on the box claims.
    Serial { tty: String },
    /// A network interface.
    Network { iface: String },
    /// A Bluetooth adapter.
    Bluetooth { hci: String },
    /// A keyboard, remote, game pad.
    Input { name: String },
    /// A sound card.
    Audio { card: String },
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UsbDevice {
    /// Where it is plugged in, e.g. "1-1.2".
    busid: String,
    /// vid:pid, e.g. "0658:0200".
    id: String,
    manufacturer: String,
    product: String,
    /// Link speed in Mbit/s ("480" for USB 2.0 high speed).
    speed: String,
    /// What the device says it is, from its class codes ("Mass storage", "Wireless").
    class: String,
    uses: Vec<Use>,
    /// Can be shared over USB/IP (the box's own radios cannot).
    shareable: bool,
    /// Shared over USB/IP right now.
    exported: bool,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UsbState {
    devices: Vec<UsbDevice>,
    /// This board has the 'usbip' feature, so devices can be shared.
    usbip: bool,
}

/// The USB class code's name (bDeviceClass, or an interface's bInterfaceClass).
fn class_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "01" => "Audio",
        "02" | "0a" => "Serial / modem",
        "03" => "Input",
        "06" => "Camera",
        "07" => "Printer",
        "08" => "Mass storage",
        "0e" => "Video",
        "e0" => "Wireless",
        "ef" | "00" => return None, // "see the interfaces"
        "ff" => "Vendor specific",
        _ => return None,
    })
}

/// Walk a device's own directory (not its children's: a hub's ports are devices
/// of their own) and note what the kernel made of it.
fn uses_in(dev: &Path, busid: &str, radios: &HashMap<String, String>, out: &mut Vec<Use>, depth: u8) {
    let Ok(rd) = std::fs::read_dir(dev) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if !ft.is_dir() {
            continue; // sysfs links (driver, subsystem, port) lead elsewhere
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if usbip::is_busid(&name) && name != busid {
            continue; // a downstream device
        }
        let parent = dev.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let found = match parent.as_str() {
            "block" => Some(Use::Storage { block: name.clone() }),
            "tty" => Some(match radios.get(&name) {
                Some(r) => Use::Radio { radio: r.clone(), tty: name.clone() },
                None => Use::Serial { tty: name.clone() },
            }),
            "net" => Some(Use::Network { iface: name.clone() }),
            "bluetooth" if name.starts_with("hci") => Some(Use::Bluetooth { hci: name.clone() }),
            "sound" if name.starts_with("card") => Some(Use::Audio { card: name.clone() }),
            "input" if name.starts_with("input") => {
                let n = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
                Some(Use::Input { name: n.trim().to_string() })
            }
            _ => None,
        };
        if let Some(u) = found {
            if !out.contains(&u) {
                out.push(u);
            }
            continue;
        }
        // usb-serial adapters put ttyUSB0 straight under the interface.
        if (name.starts_with("ttyUSB") || name.starts_with("ttyACM")) && !radios.contains_key(&name) {
            let u = Use::Serial { tty: name.clone() };
            if !out.contains(&u) {
                out.push(u);
            }
        } else if (name.starts_with("ttyUSB") || name.starts_with("ttyACM")) && radios.contains_key(&name) {
            let u = Use::Radio { radio: radios[&name].clone(), tty: name.clone() };
            if !out.contains(&u) {
                out.push(u);
            }
        }
        if depth < 8 {
            uses_in(&e.path(), busid, radios, out, depth + 1);
        }
    }
}

/// tty name -> radio kind, from the board's /dev/ohc/radio/<kind> links.
fn radio_ttys(dir: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let kind = e.file_name().to_string_lossy().into_owned();
        if let Ok(t) = std::fs::canonicalize(e.path()) {
            if let Some(n) = t.file_name() {
                m.insert(n.to_string_lossy().into_owned(), kind);
            }
        }
    }
    m
}

fn list(root: &Path, radios: &HashMap<String, String>, reserved: &std::collections::HashSet<String>) -> Vec<UsbDevice> {
    let rd = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut v: Vec<UsbDevice> = std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter_map(|e| {
            let busid = e.file_name().into_string().ok()?;
            let d = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
            if !usbip::is_busid(&busid) || rd(&d, "bDeviceClass") == "09" {
                return None; // root hubs and hubs are plumbing
            }
            let mut uses = Vec::new();
            uses_in(&d, &busid, radios, &mut uses, 0);
            let interface_class = std::fs::read_dir(&d).into_iter().flatten().flatten()
                .filter(|i| i.file_name().to_string_lossy().starts_with(&format!("{busid}:")))
                .find_map(|i| class_name(&rd(&i.path(), "bInterfaceClass")));
            let driver = std::fs::read_link(d.join("driver")).ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
            let is_radio = uses.iter().any(|u| matches!(u, Use::Radio { .. }));
            Some(UsbDevice {
                id: format!("{}:{}", rd(&d, "idVendor"), rd(&d, "idProduct")),
                manufacturer: rd(&d, "manufacturer"),
                product: rd(&d, "product"),
                speed: rd(&d, "speed"),
                class: class_name(&rd(&d, "bDeviceClass")).or(interface_class).unwrap_or("Device").to_string(),
                uses,
                shareable: !is_radio && !reserved.contains(&busid),
                exported: driver.as_deref() == Some(usbip::STUB),
                busid,
            })
        })
        .collect();
    v.sort_by(|a, b| a.busid.cmp(&b.busid));
    v
}

#[utoipa::path(get, path = "/api/usb", tag = "USB",
    summary = "What is plugged into the USB ports, and what the box does with it",
    description = "Every USB device but hubs, from sysfs: its ids and names, and what the kernel and \
the box made of it — a disk (storaged shares it), one of the box's own radios (the Zigbee or Z-Wave \
page runs it), a serial port, a network interface, a Bluetooth adapter. `shareable` and `exported` \
are its USB/IP side (see /api/usbip).",
    responses((status = 200, description = "usb", body = UsbState)))]
pub async fn usb_get(State(c): State<Arc<Config>>) -> Json<UsbState> {
    let radios = radio_ttys(Path::new("/dev/ohc/radio"));
    Json(UsbState { devices: list(Path::new(USB), &radios, &usbip::reserved(&c)), usbip: usbip::available() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fake sysfs: a flash drive, a Z-Wave stick the board named /dev/zwave,
    /// a plain serial adapter, and a hub (left out).
    #[test]
    fn names_what_each_device_is() {
        let t = std::env::temp_dir().join(format!("ohc-usb-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        let dev = |busid: &str, class: &str, vid: &str, product: &str| {
            let d = t.join(busid);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("bDeviceClass"), class).unwrap();
            fs::write(d.join("idVendor"), vid).unwrap();
            fs::write(d.join("idProduct"), "0001").unwrap();
            fs::write(d.join("product"), product).unwrap();
            d
        };
        let disk = dev("1-1", "00", "0781", "Cruzer");
        fs::create_dir_all(disk.join("1-1:1.0/host0/target0:0:0/0:0:0:0/block/sda")).unwrap();
        fs::write(disk.join("1-1:1.0/bInterfaceClass"), "08").unwrap();
        let zw = dev("1-2", "02", "0658", "Z-Stick");
        fs::create_dir_all(zw.join("1-2:1.0/tty/ttyACM0")).unwrap();
        let ser = dev("1-3", "00", "0403", "FT232R");
        fs::create_dir_all(ser.join("1-3:1.0/ttyUSB0/tty/ttyUSB0")).unwrap();
        dev("1-4", "09", "05e3", "Hub");

        let radios = HashMap::from([("ttyACM0".to_string(), "zwave".to_string())]);
        let v = list(&t, &radios, &Default::default());
        let _ = fs::remove_dir_all(&t);

        assert_eq!(v.iter().map(|d| d.busid.as_str()).collect::<Vec<_>>(), ["1-1", "1-2", "1-3"]);
        assert_eq!(v[0].uses, [Use::Storage { block: "sda".into() }]);
        assert_eq!(v[0].class, "Mass storage");
        assert_eq!(v[1].uses, [Use::Radio { radio: "zwave".into(), tty: "ttyACM0".into() }]);
        assert!(!v[1].shareable);
        assert_eq!(v[2].uses, [Use::Serial { tty: "ttyUSB0".into() }]);
        assert!(v[2].shareable);
    }
}
