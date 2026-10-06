//! Now-playing metadata from every Spotify and AirPlay endpoint.
//!
//! Each endpoint's metadata is one JSON file, `/run/ohc/audio/meta/<tag>.json`
//! (a `Meta`), which the publisher sends retained on `state/audio/meta/<tag>`;
//! `state` == "playing" is also what lights the endpoint in the map.
//!
//! * Spotify: librespot runs its `--onevent` program on every player event with
//!   the details in the environment (track_changed: NAME, ARTISTS, ALBUM,
//!   COVERS, DURATION_MS; playing/paused/stopped; session_client_changed:
//!   CLIENT_NAME). That program is a two-line script exec'ing this binary as
//!   `ohc-audiod --hook <tag>` (`hook`), which updates the file and pokes the
//!   daemon (SIGUSR1) to publish at once.
//! * AirPlay: shairport-sync writes its metadata stream — `<item>`s of a type,
//!   a code and base64 data — to a FIFO per endpoint; a reader thread per
//!   endpoint (`read_shairport`) folds it into the same file, plus the cover
//!   art beside it (`<tag>.cover`, served by `GET /api/audio/cover/<tag>`).
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub const DIR: &str = "/run/ohc/audio/meta";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Meta {
    /// playing | paused | stopped | "" (connected, nothing yet)
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub album: String,
    /// An absolute image URL (Spotify's CDN), or `cover/<tag>?v=<n>` relative
    /// to this daemon's API (AirPlay's art, served from `<tag>.cover`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cover: String,
    /// The phone or app driving it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub client: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl Meta {
    /// Nothing worth showing: no track, no sender, not playing or paused.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.artist.is_empty() && self.album.is_empty() && self.cover.is_empty()
            && self.client.is_empty() && (self.state.is_empty() || self.state == "stopped")
    }
}

pub fn path(tag: &str) -> PathBuf {
    PathBuf::from(DIR).join(format!("{tag}.json"))
}
pub fn cover_path(tag: &str) -> PathBuf {
    PathBuf::from(DIR).join(format!("{tag}.cover"))
}

pub fn load(tag: &str) -> Option<Meta> {
    serde_json::from_str(&std::fs::read_to_string(path(tag)).ok()?).ok()
}

fn store(tag: &str, m: Option<&Meta>) {
    let _ = std::fs::create_dir_all(DIR);
    match m.filter(|m| !m.is_empty()) {
        Some(m) => {
            let tmp = path(tag).with_extension("json.new");
            if serde_json::to_vec(m).ok().and_then(|b| std::fs::write(&tmp, b).ok()).is_some() {
                let _ = std::fs::rename(&tmp, path(tag));
            }
        }
        None => {
            let _ = std::fs::remove_file(path(tag));
            if m.is_none() {
                let _ = std::fs::remove_file(cover_path(tag));
            }
        }
    }
}

pub fn playing(tag: &str) -> bool {
    load(tag).is_some_and(|m| m.state == "playing")
}

/// Forget every endpoint's metadata (a restart: nothing plays yet).
pub fn clear_all() {
    let _ = std::fs::remove_dir_all(DIR);
    let _ = std::fs::create_dir_all(DIR);
}

/// Tell the running daemon something changed (SIGUSR1), so it publishes now
/// rather than on its next tick.
fn poke_daemon() {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else { return };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if pid == me {
            continue;
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim() == "ohc-audiod" {
            let _ = std::process::Command::new("kill").args(["-USR1", &pid.to_string()]).status();
        }
    }
}

/// librespot's environment for one event → the new metadata (None = forget).
pub fn apply_librespot(cur: Option<Meta>, env: &dyn Fn(&str) -> Option<String>) -> Option<Meta> {
    let mut m = cur.unwrap_or_default();
    let first_line = |k: &str| env(k).and_then(|v| v.lines().next().map(str::to_string)).unwrap_or_default();
    match env("PLAYER_EVENT").as_deref() {
        Some("track_changed") => {
            m.title = env("NAME").unwrap_or_default();
            // Tracks have ARTISTS (one per line); podcast episodes SHOW_NAME.
            let artists: Vec<String> = env("ARTISTS").unwrap_or_default().lines().map(str::to_string).collect();
            m.artist = if artists.is_empty() { env("SHOW_NAME").unwrap_or_default() } else { artists.join(", ") };
            m.album = env("ALBUM").unwrap_or_default();
            m.cover = first_line("COVERS");
            m.duration_ms = env("DURATION_MS").and_then(|d| d.parse().ok());
        }
        Some("playing") | Some("started") => m.state = "playing".into(),
        Some("paused") => m.state = "paused".into(),
        Some("stopped") => m.state = "stopped".into(),
        Some("session_client_changed") => m.client = env("CLIENT_NAME").unwrap_or_default(),
        Some("session_disconnected") => return None,
        _ => {}
    }
    Some(m)
}

/// `ohc-audiod --hook <tag>`: one librespot event.
pub fn hook(tag: &str) {
    if !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return;
    }
    let ev = std::env::var("PLAYER_EVENT").unwrap_or_default();
    // Volume moves the output's level (the endpoint's mixer): just publish.
    if ev != "volume_changed" {
        let next = apply_librespot(load(tag), &|k| std::env::var(k).ok());
        if next.as_ref() != load(tag).as_ref() {
            store(tag, next.as_ref());
        }
    }
    poke_daemon();
}

/// One `<item>` from shairport-sync's metadata stream.
#[derive(Debug, PartialEq)]
pub struct Item {
    pub kind: String,
    pub code: String,
    pub data: Vec<u8>,
}

fn hex4(s: &str) -> String {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .map(char::from)
        .collect()
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

/// Parse one `<item>…</item>` block.
pub fn parse_item(block: &str) -> Option<Item> {
    let kind = hex4(between(block, "<type>", "</type>")?.trim());
    let code = hex4(between(block, "<code>", "</code>")?.trim());
    let data = match between(block, "<data encoding=\"base64\">", "</data>") {
        Some(b64) => {
            let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).ok()?
        }
        None => Vec::new(),
    };
    Some(Item { kind, code, data })
}

/// Fold one item into the metadata. Returns whether anything changed, and
/// whether the cover art was replaced (its bytes in `cover`).
pub fn apply_shairport(m: &mut Option<Meta>, it: &Item, cover: &mut Option<Vec<u8>>, cover_ver: &mut u64, tag: &str) -> bool {
    let text = || String::from_utf8_lossy(&it.data).trim().to_string();
    let before = m.clone();
    let cur = m.get_or_insert_with(Meta::default);
    match (it.kind.as_str(), it.code.as_str()) {
        ("core", "minm") => cur.title = text(),
        ("core", "asar") => cur.artist = text(),
        ("core", "asal") => cur.album = text(),
        ("ssnc", "snam") => cur.client = text(),
        ("ssnc", "pbeg") | ("ssnc", "prsm") => cur.state = "playing".into(),
        ("ssnc", "pfls") => cur.state = "paused".into(),
        ("ssnc", "pend") => cur.state = "stopped".into(),
        ("ssnc", "PICT") => {
            if it.data.is_empty() {
                cur.cover.clear();
                *cover = None;
            } else {
                *cover_ver += 1;
                cur.cover = format!("cover/{tag}?v={cover_ver}");
                *cover = Some(it.data.clone());
            }
        }
        // The AirPlay session ended (the sender disconnected).
        ("ssnc", "aend") => *m = None,
        _ => {}
    }
    *m != before
}

/// Make the FIFO shairport-sync writes its metadata to.
pub fn fifo(tag: &str) -> PathBuf {
    PathBuf::from(DIR).join(format!("{tag}.pipe"))
}

/// Read one AirPlay endpoint's metadata FIFO forever, keeping its file up to
/// date and calling `changed` after each change.
pub fn read_shairport(tag: String, changed: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let pipe = fifo(&tag);
        let mut meta: Option<Meta> = None;
        let mut cover: Option<Vec<u8>> = None;
        let mut ver = 0u64;
        loop {
            if !pipe.exists() {
                let _ = std::fs::create_dir_all(DIR);
                let _ = std::process::Command::new("mkfifo").arg(&pipe).status();
            }
            // Read-write: on Linux that never blocks and never reads EOF, so a
            // shairport-sync restart just pauses the stream. (It opens its end
            // non-blocking, which needs a reader present — this one.)
            let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(&pipe) else {
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            };
            let mut rd = BufReader::new(f);
            let mut block = Vec::new();
            loop {
                block.clear();
                match rd.read_until(b'\n', &mut block) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                // Items span lines: gather until </item>.
                while !block.windows(7).any(|w| w == b"</item>") {
                    match rd.read_until(b'\n', &mut block) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                let Some(it) = parse_item(&String::from_utf8_lossy(&block)) else { continue };
                let had_cover = cover.is_some();
                if apply_shairport(&mut meta, &it, &mut cover, &mut ver, &tag) {
                    if let Some(c) = &cover {
                        let _ = std::fs::write(cover_path(&tag), c);
                    } else if had_cover {
                        let _ = std::fs::remove_file(cover_path(&tag));
                    }
                    store(&tag, meta.as_ref());
                    changed();
                }
            }
        }
    });
}

/// The image's media type from its first bytes.
pub fn mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if b.starts_with(b"\x89PNG") {
        "image/png"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn librespot_events_build_metadata() {
        let env = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |k: &str| m.get(k).cloned()
        };
        let t = apply_librespot(None, &env(&[
            ("PLAYER_EVENT", "track_changed"), ("NAME", "Song"), ("ARTISTS", "A\nB"), ("ALBUM", "LP"),
            ("COVERS", "https://i.scdn.co/image/big\nhttps://i.scdn.co/image/small"), ("DURATION_MS", "180000"),
        ])).unwrap();
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str()), ("Song", "A, B", "LP"));
        assert_eq!(t.cover, "https://i.scdn.co/image/big");
        assert_eq!(t.duration_ms, Some(180000));
        let p = apply_librespot(Some(t), &env(&[("PLAYER_EVENT", "playing")])).unwrap();
        assert_eq!(p.state, "playing");
        assert_eq!(p.title, "Song");
        let c = apply_librespot(Some(p), &env(&[("PLAYER_EVENT", "session_client_changed"), ("CLIENT_NAME", "Joe's iPhone")])).unwrap();
        assert_eq!(c.client, "Joe's iPhone");
        assert!(apply_librespot(Some(c), &env(&[("PLAYER_EVENT", "session_disconnected")])).is_none());
    }

    #[test]
    fn shairport_items_parse_and_fold() {
        let block = "<item><type>636f7265</type><code>6d696e6d</code><length>5</length>\n<data encoding=\"base64\">\nSGVs\nbG8=</data></item>\n";
        let it = parse_item(block).unwrap();
        assert_eq!((it.kind.as_str(), it.code.as_str(), it.data.as_slice()), ("core", "minm", &b"Hello"[..]));
        let bare = parse_item("<item><type>73736e63</type><code>70626567</code><length>0</length></item>").unwrap();
        assert_eq!((bare.kind.as_str(), bare.code.as_str()), ("ssnc", "pbeg"));

        let (mut m, mut cover, mut ver) = (None, None, 0);
        assert!(apply_shairport(&mut m, &it, &mut cover, &mut ver, "airplay-1"));
        assert!(apply_shairport(&mut m, &bare, &mut cover, &mut ver, "airplay-1"));
        let pict = Item { kind: "ssnc".into(), code: "PICT".into(), data: vec![0xff, 0xd8, 0xff] };
        assert!(apply_shairport(&mut m, &pict, &mut cover, &mut ver, "airplay-1"));
        let got = m.clone().unwrap();
        assert_eq!((got.title.as_str(), got.state.as_str(), got.cover.as_str()), ("Hello", "playing", "cover/airplay-1?v=1"));
        assert_eq!(mime(cover.as_deref().unwrap()), "image/jpeg");
        let same = Item { kind: "ssnc".into(), code: "pbeg".into(), data: vec![] };
        assert!(!apply_shairport(&mut m, &same, &mut cover, &mut ver, "airplay-1"));
        let end = Item { kind: "ssnc".into(), code: "aend".into(), data: vec![] };
        assert!(apply_shairport(&mut m, &end, &mut cover, &mut ver, "airplay-1"));
        assert!(m.is_none());
    }
}
