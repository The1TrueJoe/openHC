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
//! Percent is shairport-sync's "dasl_tapered" curve, so openHC's slider and an
//! AirPlay sender's slider are the same number: every halving is −10 dB
//! (50% → −10 dB, 25% → −20 dB, 10% → −33 dB), with a straight line to the
//! bottom of the range below ~3% where that would fall off it. It lands on
//! softvol's 0.2 dB steps rounding down, as shairport-sync's own mixer writes
//! do, so a level sent to the sender (airplay_volume) and echoed back sets the
//! same step. 0 is softvol's raw 0, which mutes.
use crate::board::{Board, Port};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// softvol resolution - 1: 0.2 dB steps from −51 dB (raw 0) to 0 dB.
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

/// The level's dB for a percent above 0 (shairport-sync's dasl_tapered_vol2attn).
fn percent_db(p: u8) -> f64 {
    let s = f64::from(p.min(100)) / 100.0;
    let tapered = 10.0 * s.log2();
    let flat = VOL_MIN_DB * (1.0 - s);
    tapered.max(flat).min(0.0)
}

pub fn percent_to_raw(p: u8) -> u32 {
    if p == 0 {
        return 0;
    }
    to_step(percent_db(p)).clamp(1, VOL_MAX)
}

/// dB → softvol step exactly as shairport-sync's mixer write lands: the dB as
/// whole hundredths (a C double → long, truncated), then ALSA's dB-scale lookup
/// for the step at or below it.
fn to_step(db: f64) -> u32 {
    let h = (db * 100.0) as i64;
    let (min, step) = ((VOL_MIN_DB * 100.0) as i64, (STEP_DB * 100.0).round() as i64);
    ((h - min).max(0) / step) as u32
}

/// The percent whose step is nearest `raw` (the highest, where several share it).
pub fn raw_to_percent(raw: u32) -> u8 {
    if raw == 0 {
        return 0;
    }
    (1..=100u8).rev().min_by_key(|&p| percent_to_raw(p).abs_diff(raw)).unwrap_or(100)
}

/// The AirPlay volume (−30 … 0, −144 = mute) whose slider position is `p`
/// percent. Through shairport-sync's dasl_tapered profile it sets exactly
/// percent_to_raw(p), so a sender told this and echoing it back changes nothing.
pub fn airplay_volume(p: u8) -> f64 {
    if p == 0 {
        return -144.0;
    }
    -30.0 + 30.0 * f64::from(p.min(100)) / 100.0
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
        assert_eq!(percent_to_raw(50), 205); // −10 dB
        assert_eq!(percent_to_raw(25), 155); // −20 dB
        assert_eq!(percent_to_raw(60), 218); // −7.4 dB, rounded down
        assert!(percent_to_raw(1) >= 1); // the flat floor, never mute
        for p in [1u8, 3, 10, 25, 50, 60, 75, 90, 100] {
            assert_eq!(raw_to_percent(percent_to_raw(p)), p, "{p}");
        }
        assert_eq!(raw_to_percent(0), 0);
    }
    #[test]
    fn airplay_volume_is_the_slider_position() {
        assert_eq!(airplay_volume(0), -144.0);
        assert_eq!(airplay_volume(100), 0.0);
        assert_eq!(airplay_volume(50), -15.0);
        // shairport-sync's dasl_tapered_vol2attn, in its own hundredths of a
        // dB, then its mixer write (the step at or below): the same raw step.
        for p in 1..=100u8 {
            let s = 1.0 + airplay_volume(p) / 30.0;
            let (max, min) = (0.0, VOL_MIN_DB * 100.0);
            let att = (max + 1000.0 * s.log10() / 2f64.log10()).max(min + (max - min) * s).min(max) / 100.0;
            assert_eq!(to_step(att).max(1), percent_to_raw(p), "{p}");
        }
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
