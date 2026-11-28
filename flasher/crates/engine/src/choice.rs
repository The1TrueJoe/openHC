//! What goes on the box: plain openHC, openHC as a Control4 controller again,
//! or one of the EA images that carry Home Assistant and Music Assistant
//! (https://github.com/The1TrueJoe/c4-ea-homeassistant). The front ends offer
//! the ones that fit the board and download the image themselves, so the user
//! never picks a file.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ohc_flash_core::board::{Board, Family};

use crate::{updates, Event, Progress};

/// The repository the Home Assistant and Music Assistant images come from.
pub const HA_REPO: &str = "The1TrueJoe/c4-ea-homeassistant";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    Openhc,
    Control4,
    HomeAssistant,
    MusicAssistant,
}

impl Install {
    pub const ALL: [Install; 4] =
        [Install::Openhc, Install::Control4, Install::HomeAssistant, Install::MusicAssistant];

    /// The CLI's spelling (`--with <name>`).
    pub fn name(self) -> &'static str {
        match self {
            Install::Openhc => "openhc",
            Install::Control4 => "control4",
            Install::HomeAssistant => "home-assistant",
            Install::MusicAssistant => "music-assistant",
        }
    }

    pub fn parse(s: &str) -> Option<Install> {
        Install::ALL.into_iter().find(|i| i.name() == s)
    }

    pub fn title(self) -> &'static str {
        match self {
            Install::Openhc => "openHC",
            Install::Control4 => "Control4 controller",
            Install::HomeAssistant => "Home Assistant",
            Install::MusicAssistant => "Music Assistant only",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Install::Openhc => {
                "openHC on its own: its web page, the IO and radios over MQTT, and AirPlay 2 \
                 (and more) on every audio output. For your own home automation."
            }
            Install::Control4 => {
                "openHC, back in your Control4 project: its IO, audio and on-screen navigator \
                 through a Composer driver, which the flasher saves to your Downloads."
            }
            Install::HomeAssistant => {
                "Home Assistant and Music Assistant on the box, with its IO, Zigbee, Bluetooth \
                 and audio outputs already in them. The first start takes 20-40 minutes."
            }
            Install::MusicAssistant => {
                "Music Assistant on the box, playing to its audio outputs, without Home \
                 Assistant. The first start takes about 15 minutes."
            }
        }
    }

    /// Whether the board can run it. The Home Assistant images exist for the EA
    /// boards only.
    pub fn fits(self, board: &Board) -> bool {
        match self {
            Install::Openhc | Install::Control4 => true,
            Install::HomeAssistant | Install::MusicAssistant => board.family == Family::Ea,
        }
    }

    /// The image's repository and its zip names' prefix.
    pub fn image(self) -> (&'static str, &'static str) {
        match self {
            Install::Openhc | Install::Control4 => (updates::REPO, "openhc"),
            Install::HomeAssistant | Install::MusicAssistant => (HA_REPO, "openhc-ha"),
        }
    }

    /// Files to put on the new root (p1) before its first boot.
    pub fn p1_files(self) -> Vec<(&'static str, &'static [u8])> {
        match self {
            // The image's Home Assistant start script stands down for this.
            Install::MusicAssistant => vec![("/etc/ohc-apps.conf", b"HOMEASSISTANT=off\n")],
            _ => vec![],
        }
    }

    /// Download the newest image for the board (kept in the temp dir, so a
    /// second install of the same image does not fetch it again).
    pub fn fetch_image(self, board: &Board, p: &Progress) -> Result<PathBuf> {
        let (repo, prefix) = self.image();
        let rel = updates::newest_from(repo)?;
        let asset = rel
            .image_asset_named(prefix, board.name)
            .with_context(|| format!("{repo} {} has no image for the {}", rel.tag_name, board.name))?;
        let dest = std::env::temp_dir().join(&asset.name);
        let have = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if asset.size == 0 || have != asset.size {
            p.emit(Event::step(format!("downloading {}", asset.name)));
            updates::download(&asset.url, &dest)?;
        }
        p.emit(Event::detail(format!("{repo} {}: {}", rel.tag_name, dest.display())));
        Ok(dest)
    }
}

/// Which Zigbee a Control4 controller's radio runs (the EA boards').
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Zigbee {
    /// openHC's zigbee-bridge: standard Zigbee 3.0 devices, on MQTT.
    #[default]
    Openhc,
    /// The radio as a Control4 Zigbee access point, for Control4's devices.
    Control4,
}

impl Zigbee {
    pub fn name(self) -> &'static str {
        match self {
            Zigbee::Openhc => "openhc",
            Zigbee::Control4 => "control4",
        }
    }

    pub fn parse(s: &str) -> Option<Zigbee> {
        [Zigbee::Openhc, Zigbee::Control4].into_iter().find(|z| z.name() == s)
    }

    /// The boards with a radio the Control4 extension can take over.
    pub fn choosable(board: &Board) -> bool {
        board.family == Family::Ea
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assistants_are_ea_only() {
        let b = |n| ohc_flash_core::board::by_name(n).unwrap();
        assert!(Install::HomeAssistant.fits(b("ea1-v2-poe")) && Install::MusicAssistant.fits(b("ea5-v1")));
        assert!(!Install::HomeAssistant.fits(b("hc800")) && !Install::MusicAssistant.fits(b("ca1")));
        assert!(Install::ALL.iter().all(|i| i.fits(b("ea3-v1"))));
        assert!(Install::ALL.iter().all(|i| Install::parse(i.name()) == Some(*i)));
    }

    #[test]
    fn only_music_assistant_writes_to_p1() {
        assert!(Install::HomeAssistant.p1_files().is_empty());
        assert_eq!(Install::MusicAssistant.p1_files()[0].0, "/etc/ohc-apps.conf");
    }
}
