//! Per-output tone: bass, treble and balance.
//!
//! The DSP is openHC's ALSA plugin (packages/alsa-ohceq, pcm type `ohceq`),
//! one stage per output between its level and its dmix (runner::asound_conf),
//! so it shapes everything on the jack — endpoints, routes, announcements. The
//! plugin creates its mixer controls on the card the first time the output is
//! opened (levels::prepare opens every output) and reads them as it plays, so
//! a change here is heard at once:
//!
//!   `<pcm> Bass Playback Volume`, `<pcm> Treble Playback Volume`  -12..12 dB
//!   `<pcm> Balance Playback Volume`  -100 (left only) .. 100 (right only)
//!
//! Nothing but this daemon writes them (unlike the level, which AirPlay and
//! Spotify also move), so the saved settings are the truth and are published
//! as they are. On an image without the plugin there is no tone stage and no
//! tone topics.
use crate::board::Port;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub const PLUGIN: &str = "/usr/lib/alsa-lib/libasound_module_pcm_ohceq.so";
pub const TONE_MAX: i32 = 12;
pub const BALANCE_MAX: i32 = 100;

pub fn available() -> bool {
    std::path::Path::new(PLUGIN).exists()
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Tone {
    /// dB, -12..12.
    pub bass: i32,
    /// dB, -12..12.
    pub treble: i32,
    /// -100 (left only) .. 100 (right only).
    pub balance: i32,
}

/// The three settings, by the name used in topics and controls.
pub const KNOBS: [&str; 3] = ["bass", "treble", "balance"];

impl Tone {
    pub fn get(&self, knob: &str) -> Option<i32> {
        match knob {
            "bass" => Some(self.bass),
            "treble" => Some(self.treble),
            "balance" => Some(self.balance),
            _ => None,
        }
    }

    /// Set one knob, clamped to its range; false for an unknown knob.
    pub fn set(&mut self, knob: &str, v: i32) -> bool {
        match knob {
            "bass" => self.bass = v.clamp(-TONE_MAX, TONE_MAX),
            "treble" => self.treble = v.clamp(-TONE_MAX, TONE_MAX),
            "balance" => self.balance = v.clamp(-BALANCE_MAX, BALANCE_MAX),
            _ => return false,
        }
        true
    }
}

pub fn control(p: &Port, knob: &str) -> String {
    let name = match knob {
        "bass" => "Bass",
        "treble" => "Treble",
        _ => "Balance",
    };
    format!("{} {name} Playback Volume", p.pcm)
}

pub fn path() -> PathBuf {
    crate::config::path().with_file_name("audio-tone.json")
}

pub fn load() -> HashMap<String, Tone> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(tones: &HashMap<String, Tone>) {
    let p = path();
    let tmp = p.with_extension("json.new");
    let r = serde_json::to_vec_pretty(tones)
        .map_err(std::io::Error::other)
        .and_then(|b| std::fs::write(&tmp, b))
        .and_then(|_| std::fs::rename(&tmp, &p));
    if let Err(e) = r {
        eprintln!("audiod: cannot save {}: {e}", p.display());
    }
}

/// Write one knob to the output's control.
pub fn apply_knob(p: &Port, knob: &str, v: i32) {
    // `--` so amixer does not read a negative value as an option.
    let st = Command::new("amixer")
        .args(["-q", "-c", &p.card(), "cset", &format!("name={}", control(p, knob)), "--", &v.to_string()])
        .status();
    if !matches!(st, Ok(s) if s.success()) {
        eprintln!("audiod: amixer cset {} {v} failed ({st:?})", control(p, knob));
    }
}

pub fn apply(p: &Port, t: &Tone) {
    for k in KNOBS {
        apply_knob(p, k, t.get(k).unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn knobs_clamp_to_their_ranges() {
        let mut t = Tone::default();
        assert!(t.set("bass", 20) && t.bass == 12);
        assert!(t.set("treble", -30) && t.treble == -12);
        assert!(t.set("balance", 250) && t.balance == 100);
        assert!(!t.set("mid", 3));
        assert_eq!(t.get("balance"), Some(100));
    }
    #[test]
    fn control_names_match_the_plugin() {
        let p = Port { id: "analog2".into(), label: "Analog 2".into(), device: "hw:CARD=Intel,DEV=2".into(), pcm: "ohc_analog2".into(), swap: false };
        assert_eq!(control(&p, "bass"), "ohc_analog2 Bass Playback Volume");
        assert_eq!(control(&p, "balance"), "ohc_analog2 Balance Playback Volume");
    }
}
