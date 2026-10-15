//! The on-screen URL: what the TV shows, if anything.
//!
//! The screen shows the splash and nothing else unless this is set; set, the
//! board's display program (webview on the EAs, cog on the HC-800) shows that
//! page full screen. S97webview does the showing; this is only the setting,
//! kept as one line in /data/ohc/webview.url so the init script can read it
//! with `head`. Empty clears it.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn url_file() -> PathBuf {
    std::env::var("OHC_WEBVIEW_URLFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/data/ohc/webview.url"))
}

const INIT: &str = "/etc/init.d/S97webview";

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct Display {
    /// The page shown on screen; empty means the splash only.
    pub url: String,
    /// Whether this image has a display program at all (read-only).
    #[serde(default)]
    pub available: bool,
}

pub fn available() -> bool {
    ["/usr/bin/webview", "/usr/bin/cog"].iter().any(|p| std::path::Path::new(p).exists())
}

pub fn get() -> Display {
    let url = std::fs::read_to_string(url_file()).unwrap_or_default();
    Display { url: url.lines().next().unwrap_or("").trim().to_string(), available: available() }
}

/// An http(s) URL the init script can pass along as one quoted argument: no
/// whitespace, quotes or control characters (it is also spliced into a shell
/// command line there).
pub fn validate(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Ok(());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("the on-screen URL must start with http:// or https://".into());
    }
    if url.len() > 2048 || url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\'' | '"' | '`' | '\\')) {
        return Err("the on-screen URL has characters it cannot carry (spaces, quotes or backslashes)".into());
    }
    Ok(())
}

/// Save the URL (or clear it) and have S97webview show it — or the splash.
pub fn set(url: &str) -> Result<Display, String> {
    let url = url.trim();
    validate(url)?;
    let f = url_file();
    let r = if url.is_empty() {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&f, format!("{url}\n"))
    };
    r.map_err(|e| format!("cannot save {}: {e}", f.display()))?;
    if std::path::Path::new(INIT).exists() {
        // Detached: a restart stops and starts the page, which takes seconds.
        let _ = std::process::Command::new("sh").arg("-c").arg(format!("{INIT} restart >/dev/null 2>&1 &")).status();
    }
    Ok(get())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_the_init_script_can_carry() {
        assert!(validate("").is_ok());
        assert!(validate("http://localhost/").is_ok());
        assert!(validate("https://ha.local:8123/lovelace/tv?kiosk").is_ok());
        assert!(validate("file:///etc/passwd").is_err());
        assert!(validate("http://x/ y").is_err());
        assert!(validate("http://x/'; reboot; '").is_err());
        assert!(validate("javascript:alert(1)").is_err());
    }
    #[test]
    fn saves_and_clears() {
        let d = std::env::temp_dir().join(format!("ohc-display-{}", std::process::id()));
        std::env::set_var("OHC_WEBVIEW_URLFILE", d.join("webview.url"));
        assert_eq!(set(" http://localhost/ ").unwrap().url, "http://localhost/");
        assert_eq!(get().url, "http://localhost/");
        assert_eq!(set("").unwrap().url, "");
        assert!(set("ftp://x").is_err());
        let _ = std::fs::remove_dir_all(d);
    }
}
