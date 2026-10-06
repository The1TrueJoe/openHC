//! Per-output volume and ducking.
//!
//! asound.conf (runner::asound_conf) puts two ALSA softvol stages in front of
//! every output's dmix, each a mixer control on the output's card:
//!
//!   `<pcm> Playback Volume`       the output's level — every endpoint, route
//!                                 and announcement on the jack goes through it
//!   `<pcm> Duck Playback Volume`  music only (`<pcm>`); announcements play to
//!                                 `<pcm>_announce`, which skips it
//!
//! The level is the one real volume of a jack. AirPlay and Spotify endpoints
//! drive it as their hardware mixer (simple control name `<pcm>`), so a phone's
//! slider moves the output's level and the level is what this daemon reports;
//! the web UI and `cmd/audio/level/<id>` set the same control.
//!
//! Percent is an audio taper, atten = 40·log10(100/p) dB (60% → −8.9 dB, 10% →
//! −40 dB), onto softvol's 0.2 dB steps; 0 is softvol's raw 0, which mutes.
use crate::board::{Board, Port};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// softvol resolution - 1: 0.2 dB steps from −51 dB (raw 1) to 0 dB.
pub const VOL_MAX: u32 = 255;
pub const VOL_MIN_DB: f64 = -51.0;
/// 0.2 dB steps from −40 dB to 0 dB. Never at 0 (mute) — ducking lowers music.
pub const DUCK_MAX: u32 = 200;
pub const DUCK_MIN_DB: f64 = -40.0;
const STEP_DB: f64 = 0.2;

pub fn vol_control(p: &Port) -> String {
    format!("{} Playback Volume", p.pcm)
}
pub fn duck_control(p: &Port) -> String {
    format!("{} Duck Playback Volume", p.pcm)
}

pub fn percent_to_raw(p: u8) -> u32 {
    if p == 0 {
        return 0;
    }
    let att = 40.0 * (100.0 / f64::from(p.min(100))).log10();
    VOL_MAX.saturating_sub((att / STEP_DB).round() as u32).max(1)
}

pub fn raw_to_percent(raw: u32) -> u8 {
    if raw == 0 {
        return 0;
    }
    let att = f64::from(VOL_MAX - raw.min(VOL_MAX)) * STEP_DB;
    (100.0 * 10f64.powf(-att / 40.0)).round().clamp(1.0, 100.0) as u8
}

/// The duck stage's raw value for `db` of attenuation.
pub fn duck_raw(db: f64) -> u32 {
    DUCK_MAX.saturating_sub((db.clamp(0.0, -DUCK_MIN_DB) / STEP_DB).round() as u32)
}

/// Saved levels: output id → percent. Live control state, but kept across a
/// reboot like any amplifier's volume knob.
pub fn path() -> PathBuf {
    crate::config::path().with_file_name("audio-levels.json")
}

pub fn load() -> HashMap<String, u8> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(levels: &HashMap<String, u8>) {
    let p = path();
    let tmp = p.with_extension("json.new");
    let r = serde_json::to_vec_pretty(levels)
        .map_err(std::io::Error::other)
        .and_then(|b| std::fs::write(&tmp, b))
        .and_then(|_| std::fs::rename(&tmp, &p));
    if let Err(e) = r {
        eprintln!("audiod: cannot save {}: {e}", p.display());
    }
}

/// Default level for an output nothing has saved: full, which is what the
/// jack played at before it had a level.
pub const DEFAULT: u8 = 100;

pub fn cset(p: &Port, control: &str, raw: u32) {
    let st = Command::new("amixer")
        .args(["-q", "-c", &p.card(), "cset", &format!("name={control}"), &raw.to_string()])
        .status();
    if !matches!(st, Ok(s) if s.success()) {
        eprintln!("audiod: amixer cset {control} {raw} failed ({st:?})");
    }
}

/// Create every output's controls and set them: softvol adds its control the
/// first time its PCM is opened, and AirPlay/Spotify need it to exist before
/// they start (they look their mixer up once). Opening the music PCM opens
/// both stages; aplay on an empty raw stream opens and closes it.
pub fn prepare(b: &Board, levels: &HashMap<String, u8>) {
    for p in &b.outputs {
        let st = Command::new("aplay")
            .args(["-q", "-t", "raw", "-f", "S16_LE", "-c", "2", "-r", &b.rate.to_string(), "-D", &p.pcm, "/dev/null"])
            .status();
        if !matches!(st, Ok(s) if s.success()) {
            eprintln!("audiod: cannot open {} to create its volume controls ({st:?})", p.pcm);
        }
        cset(p, &duck_control(p), DUCK_MAX);
        cset(p, &vol_control(p), percent_to_raw(*levels.get(&p.id).unwrap_or(&DEFAULT)));
    }
}

/// Every output's current raw level, read in one `amixer contents` per card.
pub fn read_raw(b: &Board) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let mut cards: Vec<String> = b.outputs.iter().map(Port::card).collect();
    cards.dedup();
    for card in cards {
        let Ok(o) = Command::new("amixer").args(["-c", &card, "contents"]).output() else { continue };
        let values = parse_contents(&String::from_utf8_lossy(&o.stdout));
        for p in b.outputs.iter().filter(|p| p.card() == card) {
            if let Some(v) = values.get(&vol_control(p)) {
                out.insert(p.id.clone(), *v);
            }
        }
    }
    out
}

/// `amixer contents` → control name → first channel's value.
pub fn parse_contents(text: &str) -> HashMap<String, u32> {
    let mut m = HashMap::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("numid=") {
            name = line
                .split_once(",name='")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(n, _)| n.to_string());
        } else if let (Some(n), Some(v)) = (&name, line.trim_start().strip_prefix(": values=")) {
            if let Some(Ok(x)) = v.split(',').next().map(str::parse::<u32>) {
                m.insert(n.clone(), x);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taper_round_trips_and_mutes_at_zero() {
        assert_eq!(percent_to_raw(0), 0);
        assert_eq!(percent_to_raw(100), VOL_MAX);
        assert_eq!(percent_to_raw(60), 211); // −8.8 dB
        assert_eq!(percent_to_raw(1), 1); // floor, never mute
        for p in [10u8, 25, 50, 60, 75, 90, 100] {
            assert_eq!(raw_to_percent(percent_to_raw(p)), p, "{p}");
        }
        assert_eq!(raw_to_percent(0), 0);
    }
    #[test]
    fn duck_depth() {
        assert_eq!(duck_raw(0.0), DUCK_MAX);
        assert_eq!(duck_raw(20.0), 100);
        assert_eq!(duck_raw(99.0), 0);
    }
    #[test]
    fn parses_amixer_contents() {
        let t = "numid=35,iface=MIXER,name='ohc_analog2 Playback Volume'\n  ; type=INTEGER,access=rw---RW-,values=2,min=0,max=255,step=0\n  : values=190,190\n  | dBscale-min=-51.00dB,step=0.20dB,mute=0\nnumid=7,iface=MIXER,name='Line Playback Volume'\n  : values=3,4\n";
        let m = parse_contents(t);
        assert_eq!(m.get("ohc_analog2 Playback Volume"), Some(&190));
        assert_eq!(m.get("Line Playback Volume"), Some(&3));
    }
}
