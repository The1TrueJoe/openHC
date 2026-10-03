//! Health and system state on MQTT — the web UI reads nothing over REST.
//!
//! Telemetry is sampled and kept by sysmond; it exports `now.json` (every sample)
//! and `history.json` (once a minute, downsampled to a point a minute) into
//! /run/ohc/sysmond/, and this republishes them as the retained topics
//! `health/now` and `health/history`. Fan state (`health/fan`) and the
//! return-to-stock status (`system/restore`) come from the same helpers sysmond
//! and the restore ops already use (`ohc-fand`, `ohc-restore`). The bus drops
//! unchanged values, so a steady box publishes almost nothing.
use serde_json::{json, Value};
use std::path::PathBuf;

fn export_dir() -> PathBuf {
    std::env::var_os("SYSMOND_EXPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/ohc/sysmond"))
}

fn read_json(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(export_dir().join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

/// sysmond's latest sample (`/api/now`'s shape), if it has exported one.
pub fn now() -> Option<Value> {
    read_json("now.json")
}

/// sysmond's minute-resolution history, if exported yet.
pub fn history() -> Option<Value> {
    read_json("history.json")
}

/// The fan helper — present only on boards with a controllable fan (EA).
const FAND_BIN: &str = "/opt/ohc/bin/ohc-fand";

fn fand() -> bool {
    std::path::Path::new(FAND_BIN).exists()
}

async fn run_fand(args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(FAND_BIN)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("ohc-fand: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix(key)).map(|v| v.trim().to_string())
}

/// `{ available, mode, pct }` — the same contract as sysmond's /api/fan.
pub async fn fan_status() -> Value {
    if !fand() {
        return json!({ "available": false });
    }
    match run_fand(&["status"]).await {
        Ok(out) => {
            let mode = field(&out, "mode:").unwrap_or_else(|| "auto".into());
            let pct = field(&out, "pct:").and_then(|v| v.parse::<i64>().ok());
            json!({ "available": true, "mode": mode, "pct": pct })
        }
        Err(e) => json!({ "available": true, "error": e }),
    }
}

/// Hold the fan at a manual percent (clamped 0..=100). The thermal fail-safe in
/// the fan daemon still forces 100% on a critical temperature.
pub async fn fan_set(pct: i64) -> Result<Value, String> {
    if !fand() {
        return Err("no fan control on this board".into());
    }
    let pct = pct.clamp(0, 100);
    run_fand(&["set", &pct.to_string()]).await?;
    Ok(json!({ "mode": "manual", "pct": pct }))
}

/// Release the fan back to its automatic curve.
pub async fn fan_auto() -> Result<Value, String> {
    if !fand() {
        return Err("no fan control on this board".into());
    }
    run_fand(&["auto"]).await?;
    Ok(json!({ "mode": "auto" }))
}

/// Publish everything this module owns. Called by the poller and after any
/// fan/restore action, so a change shows up without waiting for the next tick.
pub async fn publish(bus: &crate::events::Bus) {
    if let Some(v) = now() {
        bus.set("health/now", v);
    }
    if let Some(v) = history() {
        bus.set("health/history", v);
    }
    bus.set("health/fan", fan_status().await);
}
