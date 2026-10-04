//! The endpoint map — the one part of audio a user configures.
//!
//! Configuration, so it is REST (`PUT /api/audio/endpoints`), stored as JSON with
//! serde at `$OHC_DATA/ohc/audio.json` (default `/data/ohc/audio.json`, which on
//! the HC-800 is real storage — see S08ohcdata). Absent file = board.env's
//! defaults. Structured data end to end, so a name can hold any printable text
//! without escaping games.
use crate::board::{parse_list, Board};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, utoipa::ToSchema)]
pub struct Endpoint {
    /// The name a phone shows (Spotify app / AirPlay picker).
    pub name: String,
    /// Output id from the board (`analog1`, `hdmi`, …).
    pub output: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, utoipa::ToSchema)]
pub struct Route {
    /// Input id (`linein`).
    pub input: String,
    pub output: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, utoipa::ToSchema)]
#[serde(default)]
pub struct Endpoints {
    pub spotify: Vec<Endpoint>,
    pub airplay: Vec<Endpoint>,
    pub routes: Vec<Route>,
}

pub fn path() -> PathBuf {
    let data = std::env::var("OHC_DATA").unwrap_or_else(|_| "/data".into());
    let dir = PathBuf::from(&data).join("ohc");
    if dir.is_dir() {
        dir.join("audio.json")
    } else {
        PathBuf::from("/etc/ohc/audio.json")
    }
}

fn eps(var: &str) -> Vec<Endpoint> {
    parse_list(&std::env::var(var).unwrap_or_default())
        .into_iter()
        .map(|(name, output)| Endpoint { name, output })
        .collect()
}

impl Endpoints {
    /// board.env's defaults.
    pub fn defaults() -> Endpoints {
        Endpoints {
            spotify: eps("OHC_SPOTIFY"),
            airplay: eps("OHC_AIRPLAY"),
            routes: parse_list(&std::env::var("OHC_AUDIO_ROUTES").unwrap_or_default())
                .into_iter()
                .map(|(input, output)| Route { input, output })
                .collect(),
        }
    }

    /// The saved map, or the defaults when nothing has been saved (or the file
    /// does not parse — said loudly, then defaults).
    pub fn load() -> Endpoints {
        match std::fs::read_to_string(path()) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
                eprintln!("audiod: {} is not valid ({e}); using board defaults", path().display());
                Endpoints::defaults()
            }),
            Err(_) => Endpoints::defaults(),
        }
    }

    /// Write atomically (temp + rename) and fsync the directory entry.
    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &p)
    }

    /// Every name non-empty, printable and short; every output/input real.
    pub fn validate(&self, b: &Board) -> Result<(), String> {
        let name_ok = |n: &str| {
            let n = n.trim();
            !n.is_empty() && n.chars().count() <= 64 && !n.chars().any(char::is_control)
        };
        for (kind, list) in [("spotify", &self.spotify), ("airplay", &self.airplay)] {
            for e in list {
                if !name_ok(&e.name) {
                    return Err(format!("{kind}: '{}' must be 1-64 printable characters", e.name));
                }
                if b.output(&e.output).is_none() {
                    return Err(format!("{kind} '{}': no output '{}'", e.name, e.output));
                }
            }
        }
        for r in &self.routes {
            if b.input(&r.input).is_none() {
                return Err(format!("route: no input '{}'", r.input));
            }
            if b.output(&r.output).is_none() {
                return Err(format!("route: no output '{}'", r.output));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Port;
    fn board() -> Board {
        let p = |id: &str, pcm: &str| Port { id: id.into(), label: id.into(), device: "hw:0".into(), pcm: pcm.into() };
        Board { outputs: vec![p("analog1", "ohc_analog1"), p("hdmi", "ohc_hdmi")], inputs: vec![p("linein", "ohc_in_linein")], rate: 44100, helpers: vec![] }
    }
    #[test]
    fn validates_against_the_board() {
        let b = board();
        let ok = Endpoints {
            spotify: vec![Endpoint { name: "Living \"Room\" $1".into(), output: "analog1".into() }],
            airplay: vec![],
            routes: vec![Route { input: "linein".into(), output: "hdmi".into() }],
        };
        assert!(ok.validate(&b).is_ok());
        let mut bad = ok.clone();
        bad.spotify[0].output = "coax".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok.clone();
        bad.spotify[0].name = "  ".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok.clone();
        bad.spotify[0].name = "a\nb".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok;
        bad.routes[0].input = "mic".into();
        assert!(bad.validate(&b).is_err());
    }
}
