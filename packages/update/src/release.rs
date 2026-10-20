//! What a channel offers this board: GitHub's release JSON, trimmed.
//!
//! stable = the latest published release (GitHub's /releases/latest never
//! returns a pre-release); dev = the rolling `dev` pre-release images.yml
//! refreshes from main. Either way the board's image is the asset
//! `openhc-<board>-<version>.zip`.
use serde::{Deserialize, Serialize};

pub const REPO: &str = "The1TrueJoe/openHC";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Published releases.
    #[default]
    Stable,
    /// The rolling dev pre-release: main's latest build for each board.
    Dev,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Channel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Channel::Stable),
            "dev" => Some(Channel::Dev),
            _ => None,
        }
    }
    pub fn api_url(self) -> String {
        match self {
            Channel::Stable => format!("https://api.github.com/repos/{REPO}/releases/latest"),
            Channel::Dev => format!("https://api.github.com/repos/{REPO}/releases/tags/dev"),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, utoipa::ToSchema)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url", alias = "url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
pub struct GhRelease {
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// The version an asset carries if it is THIS board's image. Board names share
/// prefixes (ea1-v2 / ea1-v2-poe), so after `openhc-<board>-` what follows must
/// look like a version — a release tag (v1.2.0, 2026.10) or `<sha>-dev` — not
/// the rest of a longer board name.
pub fn version_for(asset: &str, board: &str) -> Option<String> {
    let v = asset.strip_prefix(&format!("openhc-{board}-"))?.strip_suffix(".zip")?;
    let tag = v.strip_prefix('v').unwrap_or(v);
    let is_tag = tag.chars().next().is_some_and(|c| c.is_ascii_digit());
    let is_dev = v
        .strip_suffix("-dev")
        .is_some_and(|sha| sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()));
    (is_tag || is_dev).then(|| v.to_string())
}

/// This board's image on a release, with its version.
pub fn pick(rel: &GhRelease, board: &str) -> Option<(Asset, String)> {
    rel.assets.iter().find_map(|a| version_for(&a.name, board).map(|v| (a.clone(), v)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(n: &str) -> Asset {
        Asset { name: n.into(), url: format!("https://x/{n}"), size: 1 }
    }
    #[test]
    fn picks_this_boards_image_only() {
        let rel = GhRelease {
            assets: vec![a("openhc-ea1-v2-poe-1a2b3c4-dev.zip"), a("openhc-ea1-v2-9f8e7d6-dev.zip"), a("openhc-hc800-d7893a7-dev.zip")],
        };
        assert_eq!(pick(&rel, "ea1-v2").unwrap().1, "9f8e7d6-dev");
        assert_eq!(pick(&rel, "ea1-v2-poe").unwrap().1, "1a2b3c4-dev");
        assert_eq!(pick(&rel, "hc800").unwrap().1, "d7893a7-dev");
        assert!(pick(&rel, "ca1").is_none());
    }
    #[test]
    fn release_versions() {
        assert_eq!(version_for("openhc-ca1-v1.2.0.zip", "ca1").as_deref(), Some("v1.2.0"));
        assert_eq!(version_for("openhc-ca1-2026.10.zip", "ca1").as_deref(), Some("2026.10"));
        assert!(version_for("openhc-ca1-notes.zip", "ca1").is_none());
        assert!(version_for("ohc-flasher-v1.2.0-linux", "ca1").is_none());
    }
    #[test]
    fn channels() {
        assert_eq!(Channel::parse(" Dev "), Some(Channel::Dev));
        assert!(Channel::parse("beta").is_none());
        assert!(Channel::Stable.api_url().ends_with("/releases/latest"));
        assert!(Channel::Dev.api_url().ends_with("/releases/tags/dev"));
    }
}
