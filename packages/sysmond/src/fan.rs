//! Fan control — the one place sysmond WRITES to the board.
//!
//! sysmond is otherwise read-only telemetry, but it already owns the Health
//! surface and already reads pwm, so the fan control the UI needs belongs here
//! rather than in iod: the alternative is a Health panel that reads from one
//! daemon and writes to another for no reason a user could see.
//!
//! The KERNEL runs the fan. The board's device tree makes it a pwm-fan (hwmon
//! `pwmfan`, thermal cooling device `pwm-fan`) in a thermal zone whose trips are
//! the fan curve, and the zone's step_wise governor moves it. So there is no
//! daemon to talk to, only the kernel's own files:
//!
//! * manual = the zone's `policy` switched to `user_space` (the governor stops
//!   moving the fan), then the fan's hwmon `pwm1` written;
//! * auto = `policy` back to `step_wise`.
//!
//! The mode is therefore the zone's policy, not state of ours: a restarted
//! sysmond reads it back, and a reboot comes up in auto. [`guard`] runs on every
//! sample and is what keeps manual from being dangerous (see there).
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HWMON: &str = "/sys/class/hwmon";
const THERMAL: &str = "/sys/class/thermal";
const MANUAL_POLICY: &str = "user_space";
const AUTO_POLICY: &str = "step_wise";

fn rd(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn wr(p: &Path, v: &str) -> Result<(), String> {
    std::fs::write(p, v).map_err(|e| format!("{}: {e}", p.display()))
}

fn entries(dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The kernel objects that make up the fan, found by their driver-given names.
struct Fan {
    /// hwmon dir of the pwm-fan: `pwm1` is 0-255.
    hwmon: PathBuf,
    /// Its thermal cooling device: `cur_state`/`max_state` index cooling-levels.
    cdev: Option<PathBuf>,
    /// The thermal zone that cooling device is bound to.
    zone: Option<PathBuf>,
}

impl Fan {
    /// Present only on boards whose device tree describes a pwm-fan (the EA
    /// family); absent elsewhere, which is how the UI learns not to offer the
    /// control.
    fn find() -> Option<Fan> {
        let hwmon = entries(HWMON)
            .into_iter()
            .find(|d| rd(&d.join("name")).as_deref() == Some("pwmfan"))?;
        let cdev = entries(THERMAL).into_iter().find(|d| {
            d.file_name().is_some_and(|n| n.to_string_lossy().starts_with("cooling_device"))
                && rd(&d.join("type")).as_deref() == Some("pwm-fan")
        });
        // A zone links each bound cooling device as cdevN -> ../cooling_deviceM.
        let zone = cdev.as_ref().and_then(|c| {
            let cname = c.file_name()?.to_owned();
            entries(THERMAL).into_iter().find(|z| {
                z.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone"))
                    && std::fs::read_dir(z).into_iter().flatten().flatten().any(|e| {
                        e.file_name().to_string_lossy().starts_with("cdev")
                            && std::fs::read_link(e.path())
                                .ok()
                                .and_then(|t| t.file_name().map(|f| f == cname))
                                .unwrap_or(false)
                    })
            })
        });
        Some(Fan { hwmon, cdev, zone })
    }

    fn pwm(&self) -> Option<i64> {
        rd(&self.hwmon.join("pwm1"))?.parse().ok()
    }

    fn manual(&self) -> bool {
        self.zone
            .as_ref()
            .and_then(|z| rd(&z.join("policy")))
            .is_some_and(|p| p == MANUAL_POLICY)
    }

    /// The lowest pwm that is not off, from the fan's cooling-levels in the
    /// device tree: below it the EA fan stalls and chirps rather than turning
    /// slowly, so a manual request never goes under it either.
    fn floor(&self) -> Option<i64> {
        let raw = std::fs::read(self.hwmon.join("device/of_node/cooling-levels")).ok()?;
        raw.chunks_exact(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64)
            .filter(|&v| v > 0)
            .min()
    }

    fn zone_temp(&self) -> Option<i64> {
        rd(&self.zone.as_ref()?.join("temp"))?.parse().ok()
    }

    /// Where a manual hold has to end: the zone's `hot` trip (or, without one,
    /// its `critical` trip), in milli-C.
    fn limit(&self) -> Option<i64> {
        let z = self.zone.as_ref()?;
        let trip = |kind: &str| {
            (0..32).find_map(|i| {
                (rd(&z.join(format!("trip_point_{i}_type")))? == kind)
                    .then(|| rd(&z.join(format!("trip_point_{i}_temp")))?.parse().ok())
                    .flatten()
            })
        };
        trip("hot").or_else(|| trip("critical"))
    }

    fn to_auto(&self) -> Result<(), String> {
        let Some(z) = &self.zone else {
            return Err("fan has no thermal zone to return to".into());
        };
        // Back to step_wise from FULL. The governor only moves the fan when one
        // of its per-trip targets changes, and those were frozen when manual
        // began, so a manual duty would otherwise stay until the temperature
        // happened to cross a trip. From the top it settles onto the curve as
        // the box cools: never slower than the curve, only louder for a while.
        if let Some(c) = &self.cdev {
            if let Some(max) = rd(&c.join("max_state")) {
                wr(&c.join("cur_state"), &max)?;
            }
        }
        wr(&z.join("policy"), AUTO_POLICY)
    }
}

fn pct_of(pwm: i64) -> i64 {
    (pwm * 100 + 127) / 255
}

/// `{ available, mode, pct }`. On a board with no pwm-fan this is just
/// `{ available: false }` and the panel renders nothing.
pub fn status() -> Value {
    let Some(f) = Fan::find() else {
        return json!({ "available": false });
    };
    let mode = if f.manual() { "manual" } else { "auto" };
    let mut v = json!({ "available": true, "mode": mode, "pct": f.pwm().map(pct_of) });
    if f.zone.is_none() {
        v["error"] = json!("fan is not in a thermal zone; nothing runs its curve");
    }
    v
}

/// Manual hold. Clamps to 0..=100 and lifts a non-zero request to the fan's
/// floor; reports the percent the fan was actually set to.
pub fn set(pct: i64) -> Value {
    let Some(f) = Fan::find() else {
        return json!({ "ok": false, "error": "no fan control on this board" });
    };
    if let (Some(t), Some(lim)) = (f.zone_temp(), f.limit()) {
        if t >= lim {
            return json!({ "ok": false, "error": format!("{} C is too hot for a manual hold", t / 1000) });
        }
    }
    let mut pwm = (pct.clamp(0, 100) * 255 + 50) / 100;
    if let Some(floor) = f.floor() {
        if pwm > 0 && pwm < floor {
            pwm = floor;
        }
    }
    // Policy first, so the governor cannot move the fan back in between.
    let r = f
        .zone
        .as_ref()
        .map_or(Ok(()), |z| wr(&z.join("policy"), MANUAL_POLICY))
        .and_then(|_| wr(&f.hwmon.join("pwm1"), &pwm.to_string()));
    match r {
        Ok(()) => json!({ "ok": true, "mode": "manual", "pct": pct_of(pwm) }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

/// Hand the fan back to the thermal zone's curve.
pub fn auto() -> Value {
    let Some(f) = Fan::find() else {
        return json!({ "ok": false, "error": "no fan control on this board" });
    };
    match f.to_auto() {
        Ok(()) => json!({ "ok": true, "mode": "auto" }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

/// Run on every sample. Manual must not be a way to cook the board, and the
/// kernel cannot enforce that itself while the zone is on user_space, so:
/// * a manual hold ends at the zone's hot trip, and the fan goes back to the
///   curve (which by then is at 100%);
/// * a zone the kernel disabled because its sensor stopped answering leaves the
///   fan wherever it was, so the fan goes to 100%.
///
/// Returns true when it changed something, so the caller republishes.
pub fn guard() -> bool {
    let Some(f) = Fan::find() else {
        return false;
    };
    let Some(z) = &f.zone else {
        return false;
    };
    if rd(&z.join("mode")).as_deref() == Some("disabled") {
        if f.pwm() != Some(255) {
            eprintln!("sysmond: fan: thermal zone disabled (sensor lost?) -> fan 100%");
            return wr(&f.hwmon.join("pwm1"), "255").is_ok();
        }
        return false;
    }
    if f.manual() {
        if let (Some(t), Some(lim)) = (f.zone_temp(), f.limit()) {
            if t >= lim {
                eprintln!("sysmond: fan: {} C reached the hot limit, ending the manual hold", t / 1000);
                if let Err(e) = f.to_auto() {
                    eprintln!("sysmond: fan: {e}");
                }
                return true;
            }
        }
    }
    false
}
