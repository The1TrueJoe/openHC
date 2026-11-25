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
    /// Sendspin players (sendspin-cli), which a Sendspin server finds over mDNS.
    pub sendspin: Vec<Endpoint>,
    /// Squeezelite players, which find a Logitech Media Server (or Music
    /// Assistant's Squeezelite provider) by autodiscovery.
    pub squeezelite: Vec<Endpoint>,
}

/// `$OHC_DATA/ohc` (default `/data/ohc`) when it exists: storage that survives
/// a reboot.
pub fn data_dir() -> Option<PathBuf> {
    let data = std::env::var("OHC_DATA").unwrap_or_else(|_| "/data".into());
    Some(PathBuf::from(&data).join("ohc")).filter(|d| d.is_dir())
}

pub fn path() -> PathBuf {
    data_dir().map(|d| d.join("audio.json")).unwrap_or_else(|| PathBuf::from("/etc/ohc/audio.json"))
}

impl Endpoints {
    /// One AirPlay device per output, so every jack is reachable from a phone
    /// with no setup ("HC-800 Analog 1", or just "EA-1" on a one-output board),
    /// and the library player on the first output. Spotify Connect, Sendspin
    /// and squeezelite start off; each is turned on per output (the web UI
    /// names it the same way). Inputs are not routed (one would play whatever
    /// is plugged in).
    pub fn defaults(b: &Board) -> Endpoints {
        let name = |label: &str| {
            if b.outputs.len() == 1 { b.product.clone() } else { format!("{} {label}", b.product) }
        };
        Endpoints {
            airplay: b.outputs.iter().map(|o| Endpoint { name: name(&o.label), output: o.id.clone() }).collect(),
            library: b
                .outputs
                .first()
                .map(|o| Endpoint { name: format!("{} Library", b.product), output: o.id.clone() })
                .into_iter()
                .collect(),
            ..Default::default()
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
    /// player existed still gets one, and one saved before Sendspin and
    /// squeezelite existed has them off.
    fn from_saved(text: &str, b: &Board) -> Result<Endpoints, serde_json::Error> {
        let raw: serde_json::Value = serde_json::from_str(text)?;
        let mut e: Endpoints = serde_json::from_value(raw.clone())?;
        let d = Endpoints::defaults(b);
        for (key, list, default) in [
            ("spotify", &mut e.spotify, d.spotify),
            ("airplay", &mut e.airplay, d.airplay),
            ("library", &mut e.library, d.library),
            ("sendspin", &mut e.sendspin, d.sendspin),
            ("squeezelite", &mut e.squeezelite, d.squeezelite),
        ] {
            if raw.get(key).is_none() {
                *list = default;
            }
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
        for (kind, list) in [
            ("spotify", &self.spotify),
            ("airplay", &self.airplay),
            ("library", &self.library),
            ("sendspin", &self.sendspin),
            ("squeezelite", &self.squeezelite),
        ] {
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
    fn a_missing_kind_gets_its_default_and_a_saved_one_is_kept() {
        let b = board();
        // Saved before Sendspin/squeezelite existed: those are off, the rest as
        // saved (Spotify stays on where it was).
        let old = Endpoints::from_saved(
            r#"{"spotify":[{"name":"S","output":"hdmi"}],"airplay":[],"routes":[],"library":[]}"#, &b).unwrap();
        assert_eq!(old.spotify, vec![Endpoint { name: "S".into(), output: "hdmi".into() }]);
        assert!(old.airplay.is_empty() && old.sendspin.is_empty() && old.squeezelite.is_empty());
        // Nothing saved but the routes: every kind is its default.
        assert_eq!(Endpoints::from_saved(r#"{"routes":[]}"#, &b).unwrap(), Endpoints::defaults(&b));
        let on = Endpoints::from_saved(r#"{"squeezelite":[{"name":"Q","output":"analog1"}]}"#, &b).unwrap();
        assert_eq!(on.squeezelite[0].output, "analog1");
        assert_eq!(on.airplay.len(), 2);
    }
    #[test]
    fn defaults_name_every_output_after_the_product() {
        let mut b = board();
        b.outputs[0].label = "Analog 1".into();
        b.outputs[1].label = "HDMI".into();
        let d = Endpoints::defaults(&b);
        let names: Vec<_> = d.airplay.iter().map(|e| (e.name.as_str(), e.output.as_str())).collect();
        assert_eq!(names, [("HC-800 Analog 1", "analog1"), ("HC-800 HDMI", "hdmi")]);
        // Out of the box only AirPlay (and the library player) is on.
        assert!(d.spotify.is_empty() && d.sendspin.is_empty() && d.squeezelite.is_empty());
        assert!(d.routes.is_empty());
        assert!(d.validate(&b).is_ok());
        b.outputs.truncate(1);
        b.product = "EA-1".into();
        assert_eq!(Endpoints::defaults(&b).airplay[0].name, "EA-1");
        b.outputs.clear();
        let none = Endpoints::defaults(&b);
        assert!(none.airplay.is_empty() && none.library.is_empty());
    }
    #[test]
    fn validates_against_the_board() {
        let b = board();
        let ok = Endpoints {
            spotify: vec![Endpoint { name: "Living \"Room\" $1".into(), output: "analog1".into() }],
            airplay: vec![],
            routes: vec![Route { input: "linein".into(), output: "hdmi".into() }],
            library: vec![Endpoint { name: "Library".into(), output: "analog1".into() }],
            sendspin: vec![Endpoint { name: "Den".into(), output: "hdmi".into() }],
            squeezelite: vec![Endpoint { name: "Den".into(), output: "analog1".into() }],
        };
        assert!(ok.validate(&b).is_ok());
        let mut bad = ok.clone();
        bad.squeezelite[0].output = "coax".into();
        assert!(bad.validate(&b).is_err());
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
