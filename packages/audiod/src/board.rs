//! What the board has: named outputs and inputs, from board.env.
//!
//! S92ohcaudiod exports board.env into this daemon's environment (`set -a`),
//! so these are plain environment variables — no board.env parser here:
//!
//!   OHC_AUDIO_OUTPUTS="analog1 analog2 coax hdmi"
//!   OHC_AUDIO_OUT_analog1="hw:CARD=Intel,DEV=0|Analog 1"     device|label
//!   OHC_AUDIO_INPUTS="linein"
//!   OHC_AUDIO_IN_linein="hw:CARD=Intel,DEV=0|Line in"
//!   OHC_AUDIO_RATE=44100
//!   OHC_AUDIO_HELPERS="/opt/ohc/libexec/ohc-adv7513-audio"
//!   OHC_SPOTIFY / OHC_AIRPLAY / OHC_AUDIO_ROUTES   default endpoints, "name@output;..."
//!
//! A board without OHC_AUDIO_OUTPUTS (the EA family today) has no endpoint map;
//! the daemon then only reports the receivers and handles output/volume.
use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Port {
    pub id: String,
    pub label: String,
    /// The ALSA hardware device behind it.
    pub device: String,
    /// The shared PCM endpoints play into (dmix for outputs, dsnoop for inputs).
    pub pcm: String,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub outputs: Vec<Port>,
    pub inputs: Vec<Port>,
    pub rate: u32,
    pub helpers: Vec<String>,
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

fn ports(list_var: &str, item_prefix: &str, pcm_prefix: &str) -> Vec<Port> {
    env(list_var)
        .split_whitespace()
        .filter_map(|id| {
            let raw = env(&format!("{item_prefix}{id}"));
            let (device, label) = raw.split_once('|').unwrap_or((raw.as_str(), id));
            (!device.is_empty()).then(|| Port {
                id: id.to_string(),
                label: label.to_string(),
                device: device.to_string(),
                pcm: format!("{pcm_prefix}{id}"),
            })
        })
        .collect()
}

impl Board {
    pub fn from_env() -> Board {
        Board {
            outputs: ports("OHC_AUDIO_OUTPUTS", "OHC_AUDIO_OUT_", "ohc_"),
            inputs: ports("OHC_AUDIO_INPUTS", "OHC_AUDIO_IN_", "ohc_in_"),
            rate: env("OHC_AUDIO_RATE").parse().unwrap_or(44100),
            helpers: env("OHC_AUDIO_HELPERS").split_whitespace().map(str::to_string).collect(),
        }
    }
    pub fn has_map(&self) -> bool {
        !self.outputs.is_empty()
    }
    pub fn output(&self, id: &str) -> Option<&Port> {
        self.outputs.iter().find(|p| p.id == id)
    }
    pub fn input(&self, id: &str) -> Option<&Port> {
        self.inputs.iter().find(|p| p.id == id)
    }
}

/// board.env's default lists, `"name@output;name@output"` — split on ';', the
/// LAST '@' separates name from target (so a name may contain '@').
pub fn parse_list(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .filter_map(|e| e.rsplit_once('@').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_split_on_the_last_at() {
        assert_eq!(
            parse_list(" A@analog1; me@home@hdmi ;;"),
            vec![("A".into(), "analog1".into()), ("me@home".into(), "hdmi".into())]
        );
        assert!(parse_list("no-target").is_empty());
    }
}
