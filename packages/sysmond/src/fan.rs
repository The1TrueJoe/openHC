//! Fan control — the one place sysmond WRITES to the board.
//!
//! sysmond is otherwise read-only telemetry, but it already owns the Health
//! surface and already reads pwm, so the fan control the UI needs belongs here
//! rather than in iod: the alternative is a Health panel that reads from one
//! daemon and writes to another for no reason a user could see. The actual work
//! still lives in a shell helper (`ohc-fand`) exactly the way iod routes
//! return-to-stock through `ohc-restore` — we shell out and pass its answer on.
//!
//! The EA fan's pwm is the pwm-ce5300 channel under /sys/class/pwm, NOT a hwmon
//! pwm, so the telemetry series never sees it. `ohc-fand status` is therefore
//! the authoritative source for the fan's current duty, not the sensor ring.
use serde_json::{json, Value};

/// The fan helper. Present only on boards that ship it (the EA family); absent
/// elsewhere, which is exactly how the UI learns not to offer the control — the
/// same gate `ohc-restore` gives return-to-stock.
const FAND_BIN: &str = "/opt/ohc/bin/ohc-fand";

fn available() -> bool {
    std::path::Path::new(FAND_BIN).exists()
}

/// `{ available, mode, pct }`. On a board with no fan helper this is just
/// `{ available: false }` and the panel renders nothing.
pub fn status() -> Value {
    if !available() {
        return json!({ "available": false });
    }
    match run(&["status"]) {
        Ok(out) => {
            let mode = field(&out, "mode:").unwrap_or_else(|| "auto".into());
            let pct = field(&out, "pct:").and_then(|v| v.parse::<i64>().ok());
            json!({ "available": true, "mode": mode, "pct": pct })
        }
        Err(e) => json!({ "available": true, "error": e }),
    }
}

/// Manual override. Clamps to 0..=100 before the helper ever sees it.
pub fn set(pct: i64) -> Value {
    if !available() {
        return json!({ "ok": false, "error": "no fan control on this board" });
    }
    let pct = pct.clamp(0, 100);
    match run(&["set", &pct.to_string()]) {
        Ok(_) => json!({ "ok": true, "mode": "manual", "pct": pct }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

/// Clear the override; the curve takes back over on the daemon's next pass.
pub fn auto() -> Value {
    if !available() {
        return json!({ "ok": false, "error": "no fan control on this board" });
    }
    match run(&["auto"]) {
        Ok(_) => json!({ "ok": true, "mode": "auto" }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

/// Shell out to ohc-fand and hand back stdout, mirroring iod's restore helper.
fn run(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(FAND_BIN)
        .args(args)
        .output()
        .map_err(|e| format!("ohc-fand: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Pull the value after a `key:` line out of ohc-fand's status output.
fn field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix(key))
        .map(|v| v.trim().to_string())
}
