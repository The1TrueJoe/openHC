//! The endpoint map — the one part of audio a user configures.
//!
//! Configuration, so it is REST (`PUT /api/audio/endpoints`), stored as JSON with
//! serde at `$OHC_DATA/ohc/audio.json` (default `/data/ohc/audio.json`, which on
//! the HC-800 is real storage — see S08ohcdata). Absent file = the defaults
//! below, from the product and the outputs' labels. Structured data end to end,
//! so a name can hold any printable text without escaping games.
use crate::board::Board;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, utoipa::ToSchema)]
pub struct Endpoint {
    /// The name a phone shows (Spotify app / AirPlay picker).
    pub name: String,
    /// Output id from the board (`analog1`, `hdmi`, …), or empty for an
    /// endpoint that is not connected (kept, but nothing runs).
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
    /// The music-library player (mpd, playing what is on /media — the drives
    /// storaged mounts): at most one, on the output named here.
    pub library: Vec<Endpoint>,
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

impl Endpoints {
    /// One Spotify Connect and one AirPlay device per output, so every jack is
    /// reachable from a phone with no setup ("HC-800 Analog 1", or just "EA-1"
    /// on a one-output board), and the library player on the first output.
    /// Inputs are not routed (one would play whatever is plugged in).
    pub fn defaults(b: &Board) -> Endpoints {
        let name = |label: &str| {
            if b.outputs.len() == 1 { b.product.clone() } else { format!("{} {label}", b.product) }
        };
        let each: Vec<Endpoint> =
            b.outputs.iter().map(|o| Endpoint { name: name(&o.label), output: o.id.clone() }).collect();
        Endpoints {
            spotify: each.clone(),
            airplay: each,
            routes: Vec::new(),
            library: b
                .outputs
                .first()
                .map(|o| Endpoint { name: format!("{} Library", b.product), output: o.id.clone() })
                .into_iter()
                .collect(),
        }
    }

    /// The saved map, or the defaults when nothing has been saved (or the file
    /// does not parse — said loudly, then defaults).
    pub fn load(b: &Board) -> Endpoints {
        match std::fs::read_to_string(path()) {
            Ok(t) => Endpoints::from_saved(&t, b).unwrap_or_else(|e| {
                eprintln!("audiod: {} is not valid ({e}); using board defaults", path().display());
                Endpoints::defaults(b)
            }),
            Err(_) => Endpoints::defaults(b),
        }
    }

    /// A saved map. One written before a kind of endpoint existed has no key
    /// for it, and gets the board's default for that kind (an explicitly empty
    /// list stays empty) — so a box that saved its map before the library
    /// player existed still gets one.
    fn from_saved(text: &str, b: &Board) -> Result<Endpoints, serde_json::Error> {
        let raw: serde_json::Value = serde_json::from_str(text)?;
        let mut e: Endpoints = serde_json::from_value(raw.clone())?;
        if raw.get("library").is_none() {
            e.library = Endpoints::defaults(b).library;
        }
        Ok(e)
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
        if self.library.len() > 1 {
            return Err("library: one player at most".into());
        }
        for (kind, list) in [("spotify", &self.spotify), ("airplay", &self.airplay), ("library", &self.library)] {
            for e in list {
                if !name_ok(&e.name) {
                    return Err(format!("{kind}: '{}' must be 1-64 printable characters", e.name));
                }
                // An empty output is an endpoint that is not connected: it
                // stays in the map (and the web UI's switcher) but nothing runs.
                if !e.output.is_empty() && b.output(&e.output).is_none() {
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
        let p = |id: &str, pcm: &str| Port { id: id.into(), label: id.into(), device: "hw:0".into(), pcm: pcm.into(), swap: false, ..Default::default() };
        Board {
            outputs: vec![p("analog1", "ohc_analog1"), p("hdmi", "ohc_hdmi")],
            inputs: vec![p("linein", "ohc_in_linein")],
            rate: 44100,
            helpers: vec![],
            product: "HC-800".into(),
        }
    }
    #[test]
    fn a_map_saved_before_the_library_gets_the_default() {
        let b = board();
        let old = Endpoints::from_saved(r#"{"spotify":[],"airplay":[],"routes":[]}"#, &b).unwrap();
        assert_eq!(old.library, vec![Endpoint { name: "HC-800 Library".into(), output: "analog1".into() }]);
        let none = Endpoints::from_saved(r#"{"spotify":[],"airplay":[],"routes":[],"library":[]}"#, &b).unwrap();
        assert!(none.library.is_empty());
    }
    #[test]
    fn defaults_name_every_output_after_the_product() {
        let mut b = board();
        b.outputs[0].label = "Analog 1".into();
        b.outputs[1].label = "HDMI".into();
        let d = Endpoints::defaults(&b);
        let names: Vec<_> = d.spotify.iter().map(|e| (e.name.as_str(), e.output.as_str())).collect();
        assert_eq!(names, [("HC-800 Analog 1", "analog1"), ("HC-800 HDMI", "hdmi")]);
        assert_eq!(d.airplay, d.spotify);
        assert!(d.routes.is_empty());
        assert!(d.validate(&b).is_ok());
        b.outputs.truncate(1);
        b.product = "EA-1".into();
        assert_eq!(Endpoints::defaults(&b).spotify[0].name, "EA-1");
        b.outputs.clear();
        let none = Endpoints::defaults(&b);
        assert!(none.spotify.is_empty() && none.library.is_empty());
    }
    #[test]
    fn validates_against_the_board() {
        let b = board();
        let ok = Endpoints {
            spotify: vec![Endpoint { name: "Living \"Room\" $1".into(), output: "analog1".into() }],
            airplay: vec![],
            routes: vec![Route { input: "linein".into(), output: "hdmi".into() }],
            library: vec![Endpoint { name: "Library".into(), output: "analog1".into() }],
        };
        assert!(ok.validate(&b).is_ok());
        let mut bad = ok.clone();
        bad.spotify[0].output = "coax".into();
        assert!(bad.validate(&b).is_err());
        let mut unplugged = ok.clone();
        unplugged.spotify[0].output = String::new();
        assert!(unplugged.validate(&b).is_ok());
        let mut bad = ok.clone();
        bad.spotify[0].name = "  ".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok.clone();
        bad.spotify[0].name = "a\nb".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok.clone();
        bad.routes[0].input = "mic".into();
        assert!(bad.validate(&b).is_err());
        let mut bad = ok;
        bad.library.push(Endpoint { name: "Two".into(), output: "hdmi".into() });
        assert!(bad.validate(&b).is_err());
    }
}
