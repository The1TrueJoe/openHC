//! Per-output tone: bass, treble, balance — and on a DSP board, its EQ too.
//!
//! An output's tone stage is one of two things, and each output says which by
//! the knobs it carries (`Port::tone`, filled in by `fill` once board.env is
//! read; the map publishes them so the web UI draws exactly those):
//!
//! * The software stage: openHC's ALSA plugin (packages/alsa-ohceq, pcm type
//!   `ohceq`), one per output between its level and its dmix
//!   (runner::asound_conf), so it shapes everything on the jack — endpoints,
//!   routes, announcements. The plugin creates its mixer controls on the card
//!   the first time the output is opened (levels::prepare opens every output)
//!   and reads them as it plays:
//!
//!     `<pcm> Bass Playback Volume`, `<pcm> Treble Playback Volume`  -12..12 dB
//!     `<pcm> Balance Playback Volume`  -100 (left only) .. 100 (right only)
//!
//! * The board's DSP, for a port with `dsp=<profile>` in board.env (the EA3's
//!   ADAU1451). The profile is the same file the DSP's firmware is built from
//!   (board/ea/common/firmware/dsp/adau1451/), and its knobs are ALSA controls
//!   the kernel's sigmadsp core made from that firmware: 4 bytes, big-endian,
//!   an index 0..48 into a table the DSP looks the setting up in, 24 neutral.
//!   Tone steps are the profile's `step_db`; balance is 100/24 % a step. A
//!   knob with `choices` (the DSP's output mode, its loudness) is a selector:
//!   its value is the choice's index, 0 the first, and the choice's words are
//!   what is written. No
//!   software stage sits in front of such a port. The DSP boots silent (its
//!   Volume cell at 0); once the saved level and tone are back, `run` sets the
//!   profile's run values, which is what lets it play.
//!
//! Nothing but this daemon writes them (unlike the level, which AirPlay and
//! Spotify also move), so the saved settings are the truth and are published
//! as they are, one topic per knob: `audio/<knob>/<output>`.
use crate::board::{Board, Port};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub const PLUGIN: &str = "/usr/lib/alsa-lib/libasound_module_pcm_ohceq.so";
pub const TONE_MAX: i32 = 12;
pub const BALANCE_MAX: i32 = 100;

/// A DSP knob's table: 49 entries, the middle one neutral.
const DSP_STEPS: u32 = 49;
const DSP_FLAT: u32 = 24;

pub fn available() -> bool {
    std::path::Path::new(PLUGIN).exists()
}

/// One knob an output has.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Knob {
    pub id: String,
    pub label: String,
    /// Knobs sharing a group are drawn together ("eq": the graphic equaliser).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub min: i32,
    pub max: i32,
    /// "dB"; "balance": -100 (left only) .. 100 (right only); or "choice":
    /// an index into `choices`.
    pub unit: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(skip)]
    how: How,
}

#[derive(Clone, Debug, PartialEq)]
enum How {
    /// The ohceq plugin's control: `<pcm> <suffix> Playback Volume`.
    Plugin(&'static str),
    /// A DSP control: index = 24 + value / per_index.
    Dsp { control: String, per_index: f64 },
    /// A DSP selector: each choice's bytes, for amixer.
    DspChoice { control: String, bytes: Vec<String> },
}

/// Output id's knob id → value. Every knob is 0 when neutral.
pub type Tone = HashMap<String, i32>;

pub(crate) fn plugin_knobs() -> Vec<Knob> {
    let k = |id: &str, label: &str, suffix, min, max, unit| Knob {
        id: id.into(), label: label.into(), group: None, min, max, unit, choices: Vec::new(), how: How::Plugin(suffix),
    };
    vec![
        k("bass", "Bass", "Bass", -TONE_MAX, TONE_MAX, "dB"),
        k("treble", "Treble", "Treble", -TONE_MAX, TONE_MAX, "dB"),
        k("balance", "Balance", "Balance", -BALANCE_MAX, BALANCE_MAX, "balance"),
    ]
}

#[derive(Deserialize)]
struct Profile {
    step_db: f64,
    knobs: Vec<ProfileKnob>,
    #[serde(default)]
    cells: Vec<ProfileCell>,
}

/// A fixed cell: the firmware writes its `value` at boot; `run`, if set, is
/// what this daemon writes once the output's level and tone are restored.
#[derive(Deserialize)]
struct ProfileCell {
    control: String,
    #[serde(default)]
    run: Option<u32>,
}

#[derive(Deserialize)]
struct ProfileKnob {
    id: String,
    label: String,
    control: String,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    balance: Option<serde_json::Value>,
    #[serde(default)]
    choices: Vec<ProfileChoice>,
}

#[derive(Deserialize)]
struct ProfileChoice {
    label: String,
    words: Vec<serde_json::Value>,
}

/// A profile word as it goes on the wire: an 8.24 number, or "0x..." raw.
fn word(v: &serde_json::Value) -> Result<u32, String> {
    match v {
        serde_json::Value::String(s) => {
            u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| format!("word {s}: {e}"))
        }
        v => v.as_f64().map(|x| (x * f64::from(1u32 << 24)).round() as i32 as u32).ok_or_else(|| format!("word {v}")),
    }
}

fn dsp_knobs(profile: &str) -> Result<Vec<Knob>, String> {
    let p: Profile = serde_json::from_str(profile).map_err(|e| e.to_string())?;
    let half = DSP_FLAT as f64;
    p.knobs
        .into_iter()
        .map(|k| {
            if !k.choices.is_empty() {
                let bytes = k.choices.iter()
                    .map(|c| c.words.iter().map(word).collect::<Result<Vec<_>, _>>().map(|w| w.iter().map(|x| be_bytes(*x)).collect::<Vec<_>>().join(",")))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Knob {
                    id: k.id, label: k.label, group: k.group, min: 0, max: bytes.len() as i32 - 1, unit: "choice",
                    choices: k.choices.into_iter().map(|c| c.label).collect(),
                    how: How::DspChoice { control: k.control, bytes },
                });
            }
            let (max, unit, per_index) = if k.balance.is_some() {
                (BALANCE_MAX, "balance", BALANCE_MAX as f64 / half)
            } else {
                ((half * p.step_db) as i32, "dB", p.step_db)
            };
            Ok(Knob { id: k.id, label: k.label, group: k.group, min: -max, max, unit, choices: Vec::new(), how: How::Dsp { control: k.control, per_index } })
        })
        .collect()
}

/// The knobs a port has: its DSP profile's, else the plugin's when the image
/// has it, else none.
pub fn knobs(p: &Port) -> Vec<Knob> {
    match &p.dsp {
        Some(path) => match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| dsp_knobs(&t)) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("audiod: {}: DSP profile {path}: {e}; no tone controls", p.id);
                Vec::new()
            }
        },
        None if available() => plugin_knobs(),
        None => Vec::new(),
    }
}

/// A DSP profile's run values: (control, bytes for amixer).
fn dsp_run(profile: &str) -> Result<Vec<(String, String)>, String> {
    let p: Profile = serde_json::from_str(profile).map_err(|e| e.to_string())?;
    Ok(p.cells
        .into_iter()
        .filter_map(|c| c.run.map(|v| (c.control, be_bytes(v))))
        .collect())
}

fn be_bytes(v: u32) -> String {
    v.to_be_bytes().map(|b| b.to_string()).join(",")
}

/// Bring a DSP output up to playing: its run values, after the level and tone.
pub fn run(p: &Port) {
    let Some(path) = &p.dsp else { return };
    match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| dsp_run(&t)) {
        Ok(cells) => {
            for (name, value) in cells {
                cset(p, &name, &value);
            }
        }
        Err(e) => eprintln!("audiod: {}: DSP profile {path}: {e}", p.id),
    }
}

/// Give every output its knobs.
pub fn fill(b: &mut Board) {
    for p in &mut b.outputs {
        p.tone = knobs(p);
    }
}

/// Does this output's tone need the software stage in front of it?
pub fn plugin_stage(p: &Port) -> bool {
    p.tone.iter().any(|k| matches!(k.how, How::Plugin(_)))
}

pub fn knob<'a>(p: &'a Port, id: &str) -> Option<&'a Knob> {
    p.tone.iter().find(|k| k.id == id)
}

/// The ALSA control behind a knob, and the value to write to it.
fn control(p: &Port, k: &Knob, v: i32) -> (String, String) {
    match &k.how {
        How::Plugin(suffix) => (format!("{} {suffix} Playback Volume", p.pcm), v.to_string()),
        How::Dsp { control, per_index } => {
            let i = (f64::from(DSP_FLAT) + f64::from(v) / per_index).round().clamp(0.0, f64::from(DSP_STEPS - 1)) as u32;
            (control.clone(), be_bytes(i))
        }
        How::DspChoice { control, bytes } => (control.clone(), bytes[v.clamp(0, bytes.len() as i32 - 1) as usize].clone()),
    }
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

fn cset(p: &Port, name: &str, value: &str) {
    // `--` so amixer does not read a negative value as an option.
    let st = Command::new("amixer")
        .args(["-q", "-c", &p.card(), "cset", &format!("name={name}"), "--", value])
        .status();
    if !matches!(st, Ok(s) if s.success()) {
        eprintln!("audiod: amixer cset {name} {value} failed ({st:?})");
    }
}

/// Write one knob to its control.
pub fn apply_knob(p: &Port, k: &Knob, v: i32) {
    let (name, value) = control(p, k, v);
    cset(p, &name, &value);
}

pub fn apply(p: &Port, t: &Tone) {
    for k in &p.tone {
        apply_knob(p, k, t.get(&k.id).copied().unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EA3: &str = include_str!("../../../board/ea/common/firmware/dsp/adau1451/ea3.json");
    const EA3_OPENHC: &str = include_str!("../../../board/ea/common/firmware/dsp/adau1451/ea3-openhc.json");

    fn port(tone: Vec<Knob>) -> Port {
        Port { id: "analog".into(), label: "Analog".into(), device: "hw:CARD=openhcea,DEV=0".into(), pcm: "ohc_analog".into(), tone, ..Default::default() }
    }
    #[test]
    fn plugin_controls_match_the_plugin() {
        let p = Port { pcm: "ohc_analog2".into(), ..port(plugin_knobs()) };
        assert_eq!(control(&p, knob(&p, "bass").unwrap(), -3), ("ohc_analog2 Bass Playback Volume".into(), "-3".into()));
        assert_eq!(control(&p, knob(&p, "balance").unwrap(), 100).0, "ohc_analog2 Balance Playback Volume");
        assert!(plugin_stage(&p));
    }
    #[test]
    fn the_ea3_profile_has_tone_balance_and_ten_bands() {
        let k = dsp_knobs(EA3).unwrap();
        let ids: Vec<&str> = k.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(&ids[..5], ["mode", "loudness", "bass", "treble", "balance"]);
        assert_eq!(k.iter().filter(|k| k.group.as_deref() == Some("eq")).count(), 10);
        assert_eq!((k[0].min, k[0].max, k[0].unit), (0, 3, "choice"));
        assert_eq!(k[0].choices, ["Tone", "EQ", "Flat", "Test tone"]);
        assert_eq!((k[2].min, k[2].max, k[2].unit), (-12, 12, "dB"));
        assert_eq!((k[4].min, k[4].max, k[4].unit), (-100, 100, "balance"));
        assert!(!plugin_stage(&port(k)));
    }
    #[test]
    fn dsp_values_become_big_endian_table_indices() {
        let p = port(dsp_knobs(EA3).unwrap());
        let bass = knob(&p, "bass").unwrap();
        assert_eq!(control(&p, bass, 0), ("DSP Bass".into(), "0,0,0,24".into()));
        assert_eq!(control(&p, bass, 12).1, "0,0,0,48");
        assert_eq!(control(&p, bass, -12).1, "0,0,0,0");
        assert_eq!(control(&p, bass, -40).1, "0,0,0,0");
        let bal = knob(&p, "balance").unwrap();
        assert_eq!(control(&p, bal, 100).1, "0,0,0,48");
        assert_eq!(control(&p, bal, -50).1, "0,0,0,12");
        assert_eq!(control(&p, knob(&p, "eq1k").unwrap(), 3), ("DSP EQ 1 kHz".into(), "0,0,0,30".into()));
    }
    #[test]
    fn dsp_selectors_write_their_choice_words() {
        let p = port(dsp_knobs(EA3).unwrap());
        let mode = knob(&p, "mode").unwrap();
        // EQ: one-hot index 2 on both channels, 1.0 in 8.24.
        assert_eq!(control(&p, mode, 1).1, "0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0");
        assert_eq!(control(&p, mode, 9).1, control(&p, mode, 3).1);
        assert_eq!(control(&p, knob(&p, "loudness").unwrap(), 1), ("DSP Loudness".into(), "0,0,0,0,1,0,0,0,0,0,0,0,1,0,0,0".into()));
        assert_eq!(word(&serde_json::json!("0x00fffe20")).unwrap(), 0x00fffe20);
        assert_eq!(word(&serde_json::json!(-0.5)).unwrap(), 0xff800000);
    }
    #[test]
    fn the_ea3_dsp_boots_silent_and_audiod_raises_its_volume() {
        assert_eq!(dsp_run(EA3).unwrap(), vec![("DSP Volume".to_string(), "0,0,0,100".to_string())]);
        let p: serde_json::Value = serde_json::from_str(EA3).unwrap();
        let vol = p["cells"].as_array().unwrap().iter().find(|c| c["control"] == "DSP Volume").unwrap();
        assert_eq!(vol["value"], 0);
    }
    #[test]
    fn openhcs_own_program_has_the_same_knobs() {
        let stock: Vec<String> = dsp_knobs(EA3).unwrap().into_iter().map(|k| k.id).collect();
        let k = dsp_knobs(EA3_OPENHC).unwrap();
        assert_eq!(k.iter().map(|k| k.id.clone()).collect::<Vec<_>>(), stock);
        assert_eq!(k[0].choices, ["Tone", "EQ", "Flat", "Test tone"]);
        let p = port(k);
        // Its selectors are mux indices, its filter knobs table indices.
        assert_eq!(control(&p, knob(&p, "mode").unwrap(), 1), ("DSP Output Mode".into(), "0,0,0,1".into()));
        assert_eq!(control(&p, knob(&p, "loudness").unwrap(), 1).1, "0,0,0,1");
        assert_eq!(control(&p, knob(&p, "bass").unwrap(), 0).1, "0,0,0,24");
        assert_eq!(control(&p, knob(&p, "balance").unwrap(), -50).1, "0,0,0,12");
        // Its volume is a linear gain per channel: 0 at boot, 1.0 to run.
        assert_eq!(dsp_run(EA3_OPENHC).unwrap(), vec![
            ("DSP Volume Left".to_string(), "1,0,0,0".to_string()),
            ("DSP Volume Right".to_string(), "1,0,0,0".to_string()),
        ]);
    }
    #[test]
    fn an_old_saved_tone_still_loads() {
        let t: HashMap<String, Tone> = serde_json::from_str(r#"{"analog1":{"bass":3,"treble":0,"balance":-20}}"#).unwrap();
        assert_eq!(t["analog1"]["bass"], 3);
    }
}
