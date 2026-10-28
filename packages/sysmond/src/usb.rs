//! Rear USB over-current recovery — sysmond's other write to the board.
//!
//! The EA's rear USB port gets its 5 V through a load switch. The switch limits
//! the current itself and pulls its fault flag low while it does; Control4's
//! stock kernel (the closed c4overcurrent driver) caught the falling edge of
//! that flag, cut the switch for 2500 jiffies and powered the port back up,
//! counting the faults. This is the same recovery, done from here:
//!
//! * the flag is the GPIO line the board device tree names `usb_rear_oc`;
//! * the port is the one on the controller whose device-tree node has a
//!   `vbus-supply` — the board-switched VBUS that ehci-pci turns with root-hub
//!   port power (patches/linux/0016), so this is the only port whose power the
//!   recovery can actually cut;
//! * the cut is the port's `disable` attribute, so usbcore disconnects the
//!   device first and then switches the regulator, rather than having its
//!   power pulled out from under it.
//!
//! WHY NOT IN THE KERNEL. Nothing upstream does this: regulator-fixed's only
//! interrupt is a system-critical under-voltage, and ehci-pci has no way to be
//! told about an over-current flag on a GPIO. Teaching it one means a bigger
//! patch 0016, where the goal is fewer patches. And the CE5300 GPIO driver has
//! no interrupt support, so a kernel consumer would be polling too.
//!
//! WHY A POLL. Same reason: no interrupt, so no edge events through the GPIO
//! character device either. A 50 ms poll of one line is cheap, and missing a
//! shorter blip costs nothing — the switch is already limiting the current;
//! cutting it is recovery, not protection. Edge semantics are kept: only a
//! high-to-low change counts, so a flag stuck low from boot leaves the port
//! powered, as it did under the stock driver's falling-edge interrupt.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const USB_DEVICES: &str = "/sys/bus/usb/devices";

/// What `health/usb` and /api/usb report. `cut` is true while recovery holds
/// the port off.
#[derive(Default)]
pub struct Status {
    available: bool,
    faults: u64,
    cut: bool,
    last_fault: Option<u64>,
}

pub type Shared = Arc<Mutex<Status>>;

/// `{ available, faults, cut, last_fault }`; `{ available: false }` on a board
/// with no switched port to recover.
pub fn json(s: &Shared) -> Value {
    let s = s.lock().unwrap();
    if !s.available {
        return json!({ "available": false });
    }
    json!({ "available": true, "faults": s.faults, "cut": s.cut, "last_fault": s.last_fault })
}

/// The `disable` attribute of the board-switched port: port 1 of the root hub
/// whose controller's device-tree node carries a `vbus-supply`.
fn port() -> Option<PathBuf> {
    let mut hubs: Vec<PathBuf> = std::fs::read_dir(USB_DEVICES)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("usb")))
        .collect();
    hubs.sort();
    hubs.into_iter().find_map(|hub| {
        let hub = std::fs::canonicalize(&hub).ok()?;
        if !hub.parent()?.join("of_node/vbus-supply").exists() {
            return None;
        }
        let bus = hub.file_name()?.to_string_lossy().trim_start_matches("usb").to_string();
        let disable = hub.join(format!("{bus}-0:1.0/usb{bus}-port1/disable"));
        disable.exists().then_some(disable)
    })
}

#[cfg(target_os = "linux")]
mod watch {
    use super::*;
    use gpiocdev::{line::Value as Level, Request};
    use std::time::Duration;

    const OC_LINE: &str = "usb_rear_oc";
    const POLL: Duration = Duration::from_millis(50);
    /// The stock driver's 2500 jiffies, taken as HZ=1000.
    const OFF: Duration = Duration::from_millis(2500);

    fn unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn wr(p: &std::path::Path, v: &str) {
        if let Err(e) = std::fs::write(p, v) {
            eprintln!("sysmond: usb: {}: {e}", p.display());
        }
    }

    /// Start watching, or do nothing on a board without the line or the port.
    pub fn spawn(status: Shared, changed: Arc<tokio::sync::Notify>) {
        let Some(line) = gpiocdev::find_named_line(OC_LINE) else {
            return;
        };
        if port().is_none() {
            eprintln!("sysmond: usb: {OC_LINE} found but no board-switched port to cut");
            return;
        }
        // Active-low: Active is the switch reporting a fault.
        let req = match Request::builder()
            .with_found_line(&line)
            .with_consumer("sysmond")
            .as_input()
            .as_active_low()
            .request()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("sysmond: usb: cannot claim {OC_LINE}: {e}");
                return;
            }
        };
        let offset = line.info.offset;
        status.lock().unwrap().available = true;
        changed.notify_one();
        eprintln!("sysmond: usb: watching {OC_LINE} for rear-port over-current");

        let fault = move |r: &Request| r.value(offset).map(|v| v == Level::Active).unwrap_or(false);
        let spawned = std::thread::Builder::new().name("usb-oc".into()).spawn(move || {
            let mut was = fault(&req);
            loop {
                std::thread::sleep(POLL);
                let now = fault(&req);
                if !now || was {
                    was = now;
                    continue;
                }
                // Looked up again: the controller may have been re-probed.
                // Already disabled means someone switched the port off
                // themselves, and it is theirs to switch back on.
                let Some(disable) = port() else { continue };
                if std::fs::read_to_string(&disable).is_ok_and(|v| v.trim() == "1") {
                    was = now;
                    continue;
                }
                {
                    let mut s = status.lock().unwrap();
                    s.faults += 1;
                    s.cut = true;
                    s.last_fault = Some(unix());
                    eprintln!(
                        "sysmond: usb: rear port over-current (#{}), power off for {} ms",
                        s.faults,
                        OFF.as_millis()
                    );
                }
                changed.notify_one();
                wr(&disable, "1");
                std::thread::sleep(OFF);
                // Re-arm on the flag as it reads with the port still off, as
                // an edge interrupt would: if the switch drops it when
                // disabled, a fault that is still there after power-up is a
                // new edge and cuts again.
                was = fault(&req);
                wr(&disable, "0");
                status.lock().unwrap().cut = false;
                changed.notify_one();
            }
        });
        if let Err(e) = spawned {
            eprintln!("sysmond: usb: cannot start watcher: {e}");
        }
    }
}

#[cfg(target_os = "linux")]
pub use watch::spawn;

/// GPIO is Linux-only; on the maintainer's Mac there is nothing to watch.
#[cfg(not(target_os = "linux"))]
pub fn spawn(_status: Shared, _changed: Arc<tokio::sync::Notify>) {
    let _ = port;
}
