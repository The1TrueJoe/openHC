//! Pull openHC images and flasher updates from GitHub Releases.
//!
//! No HTTP crate: downloads and API calls shell out to `curl`, exactly as the
//! transport crate shells out to `ssh` — it ships on macOS, Windows 10+ and
//! Linux. The JSON that comes back is parsed with the serde stack the workspace
//! already carries. Releases are public, so none of this needs a token.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// The repository releases are published from. One place, so a fork changes it
/// once.
pub const REPO: &str = "The1TrueJoe/openHC";

/// One downloadable file on a release.
#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

/// A GitHub release, trimmed to the fields we use.
#[derive(Debug, Clone, Deserialize)]
pub struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl GhRelease {
    /// The image bundle for a board: `openhc-<board>-<version>.zip`.
    pub fn image_asset(&self, board: &str) -> Option<&Asset> {
        let prefix = format!("openhc-{board}-");
        self.assets
            .iter()
            .find(|a| a.name.starts_with(&prefix) && a.name.ends_with(".zip"))
    }

    /// The flasher build for the OS this binary is running on:
    /// `ohc-flasher-<version>-<os>[.exe]`.
    pub fn flasher_asset(&self) -> Option<&Asset> {
        let tag = format!("-{}", this_os());
        self.assets
            .iter()
            .find(|a| a.name.starts_with("ohc-flasher-") && a.name.contains(&tag))
    }
}

/// `macos` / `windows` / `linux` — matches the names the CI bundles use.
pub fn this_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// The latest published (non-prerelease, non-draft) release.
pub fn latest_release() -> Result<GhRelease> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-A",
            "ohc-flasher",
            "-H",
            "Accept: application/vnd.github+json",
            &url,
        ])
        .output()
        .context("running curl (is it installed?)")?;
    if !out.status.success() {
        // 404 here usually just means no release has been published yet.
        bail!(
            "no published release found for {REPO} (curl exit {})",
            out.status.code().unwrap_or(-1)
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing the GitHub release JSON")
}

/// Download one asset to `dest`. `-f` fails on an HTTP error instead of writing
/// the error page to the file; `-L` follows the redirect to the CDN.
pub fn download(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args(["-fL", "-A", "ohc-flasher", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .context("running curl (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "download failed (curl exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Resolve a downloaded flasher asset to the executable to install.
///
/// macOS ships the flasher as a zipped `.app` bundle (for the Finder icon and
/// double-click launch), so the actual Mach-O lives at `Contents/MacOS/...`
/// inside it — extract that. Windows and Linux ship the bare binary already, so
/// the path passes straight through.
pub fn resolve_executable(downloaded: &Path) -> Result<PathBuf> {
    if !downloaded.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        return Ok(downloaded.to_path_buf());
    }
    let f = std::fs::File::open(downloaded)
        .with_context(|| format!("open {}", downloaded.display()))?;
    let mut zip = zip::ZipArchive::new(f).context("reading the downloaded .app bundle")?;
    let idx = (0..zip.len())
        .find(|&i| {
            zip.by_index(i)
                .map(|e| {
                    let n = e.name();
                    e.is_file()
                        && n.contains("/Contents/MacOS/")
                        && !n.contains("__MACOSX")
                        && !n.rsplit('/').next().unwrap_or("").starts_with("._")
                })
                .unwrap_or(false)
        })
        .context("no Contents/MacOS/ executable inside the downloaded bundle")?;
    let mut entry = zip.by_index(idx)?;
    let name = entry.name().rsplit('/').next().unwrap_or("ohc-flasher").to_string();
    let dest = std::env::temp_dir().join(name);
    let mut out = std::fs::File::create(&dest)
        .with_context(|| format!("writing {}", dest.display()))?;
    std::io::copy(&mut entry, &mut out).context("extracting the app binary")?;
    Ok(dest)
}

/// Does `installed` differ from the latest release `tag`? Deliberately an
/// inequality, not an ordering: versions are either a release tag or a
/// `<sha>-dev` string, which have no total order, so the honest signal a front
/// end can give is "this box is not on the current release" — true for an older
/// release and for any dev build. Empty/unknown installed versions never nag.
pub fn differs_from_release(installed: &str, tag: &str) -> bool {
    let i = installed.trim();
    !i.is_empty() && i != "unknown" && i != tag.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel() -> GhRelease {
        serde_json::from_str(
            r#"{"tag_name":"v1.2.0","assets":[
                {"name":"openhc-ea3-v2-v1.2.0.zip","browser_download_url":"http://x/a","size":10},
                {"name":"openhc-ca1-v1.2.0.zip","browser_download_url":"http://x/b","size":10},
                {"name":"ohc-flasher-v1.2.0-macos.zip","browser_download_url":"http://x/c","size":10},
                {"name":"ohc-flasher-v1.2.0-windows.exe","browser_download_url":"http://x/d","size":10}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn picks_the_right_board_bundle() {
        assert_eq!(rel().image_asset("ea3-v2").unwrap().name, "openhc-ea3-v2-v1.2.0.zip");
        assert_eq!(rel().image_asset("ca1").unwrap().name, "openhc-ca1-v1.2.0.zip");
        assert!(rel().image_asset("hc800").is_none());
    }

    #[test]
    fn resolve_passes_a_bare_binary_through() {
        let p = std::path::Path::new("/tmp/ohc-flasher-x-linux");
        assert_eq!(resolve_executable(p).unwrap(), p);
    }

    #[test]
    fn resolve_extracts_the_app_binary_from_a_zip() {
        // Build a minimal .app zip in a temp file, then pull the binary out.
        let zip_path = std::env::temp_dir().join("ohc-resolve-test.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            use std::io::Write;
            w.start_file("ohc-flasher.app/Contents/MacOS/ohc-flasher", opts).unwrap();
            w.write_all(b"\x7fELF-not-really").unwrap();
            w.finish().unwrap();
        }
        let out = resolve_executable(&zip_path).unwrap();
        assert_eq!(out.file_name().unwrap(), "ohc-flasher");
        assert_eq!(std::fs::read(&out).unwrap(), b"\x7fELF-not-really");
        let _ = std::fs::remove_file(&zip_path);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn update_signal_is_an_honest_inequality() {
        assert!(differs_from_release("abc1234-dev", "v1.2.0"));
        assert!(differs_from_release("v1.1.0", "v1.2.0"));
        assert!(!differs_from_release("v1.2.0", "v1.2.0"));
        assert!(!differs_from_release("", "v1.2.0")); // unknown: no nag
        assert!(!differs_from_release("unknown", "v1.2.0"));
    }
}
