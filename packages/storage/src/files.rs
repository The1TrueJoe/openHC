//! Files on the mounted drives, for the web UI's file browser.
//!
//! Every path is `<volume>/<path inside it>` — a volume being one of the
//! shares ohc-storaged has mounted (`/media/<volume>`). Paths are resolved
//! component by component (no `..`, no absolute paths, no NUL), and the result
//! must still be inside that volume after following symlinks, so nothing here
//! can reach outside the drives. Writes are refused on a read-only volume.
//!
//! Uploads and downloads stream: a multi-gigabyte file never sits in memory.
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Serialize;
use serde_json::json;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;
use tokio::io::AsyncWriteExt;

pub fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, axum::Json(json!({ "error": msg.into() }))).into_response()
}

/// `<volume>/<rest>` → (absolute path, volume), if it stays inside the volume.
/// `volumes` are the mounted volumes' names with whether each is read-only;
/// `media` is where they are mounted (/media).
pub fn resolve(media: &Path, path: &str, volumes: &[(String, bool)], write: bool) -> Result<(PathBuf, String), Response> {
    let path = path.trim_matches('/');
    if path.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "bad path"));
    }
    let mut parts = path.splitn(2, '/');
    let vol = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let Some((_, ro)) = volumes.iter().find(|(n, _)| n == vol) else {
        return Err(err(StatusCode::NOT_FOUND, format!("no volume '{vol}'")));
    };
    if write && *ro {
        return Err(err(StatusCode::FORBIDDEN, format!("'{vol}' is mounted read-only")));
    }
    let root = media.join(vol);
    let mut p = root.clone();
    for c in Path::new(rest).components() {
        match c {
            Component::Normal(n) => p.push(n),
            Component::CurDir => {}
            _ => return Err(err(StatusCode::BAD_REQUEST, "bad path")),
        }
    }
    // Follow symlinks for whatever exists; it must still be on the volume.
    let existing = p.ancestors().find(|a| a.exists()).map(Path::to_path_buf).unwrap_or_else(|| root.clone());
    let real_root = std::fs::canonicalize(&root).unwrap_or(root);
    if !std::fs::canonicalize(&existing).is_ok_and(|r| r.starts_with(&real_root)) {
        return Err(err(StatusCode::BAD_REQUEST, "path leaves the volume"));
    }
    Ok((p, vol.to_string()))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: u64,
}

pub fn list(dir: &Path) -> Result<Vec<Entry>, Response> {
    let rd = std::fs::read_dir(dir).map_err(|e| err(StatusCode::NOT_FOUND, format!("{e}")))?;
    let mut v: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let md = e.metadata().ok()?;
            Some(Entry {
                name,
                dir: md.is_dir(),
                size: if md.is_dir() { 0 } else { md.len() },
                modified: md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()),
            })
        })
        .collect();
    // Folders first, then by name, case-insensitively — what a file manager does.
    v.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

/// Stream a file out as an attachment.
pub async fn download(p: &Path) -> Response {
    let f = match tokio::fs::File::open(p).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::NOT_FOUND, format!("{e}")),
    };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
    // RFC 6266: an ASCII fallback plus the exact name, percent-encoded.
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", pct(&name));
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(f)),
    )
        .into_response()
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Stream a request body into `p` (through a temporary file beside it, so an
/// interrupted upload never leaves a truncated file under the real name).
pub async fn upload(p: &Path, body: Body, overwrite: bool) -> Response {
    if p.exists() && !overwrite {
        return err(StatusCode::CONFLICT, "a file with that name exists");
    }
    if p.is_dir() {
        return err(StatusCode::CONFLICT, "a folder with that name exists");
    }
    let tmp = p.with_file_name(format!(".{}.upload", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    let mut f = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
    };
    let mut body = body;
    let mut n: u64 = 0;
    while let Some(frame) = body.frame().await {
        let chunk = match frame {
            Ok(fr) => match fr.into_data() {
                Ok(d) => d,
                Err(_) => continue, // trailers
            },
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return err(StatusCode::BAD_REQUEST, format!("upload interrupted: {e}"));
            }
        };
        if let Err(e) = f.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(StatusCode::INSUFFICIENT_STORAGE, format!("write failed (drive full?): {e}"));
        }
        n += chunk.len() as u64;
    }
    if let Err(e) = f.sync_all().await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    drop(f);
    if let Err(e) = tokio::fs::rename(&tmp, p).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"));
    }
    axum::Json(json!({ "ok": true, "bytes": n })).into_response()
}

/// A single new name inside a folder: not empty, no separators, not `.`/`..`.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n != "." && n != ".." && n.len() <= 255 && !n.contains('/') && !n.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vols() -> Vec<(String, bool)> {
        vec![("Stick".into(), false), ("Locked".into(), true)]
    }
    #[test]
    fn paths_stay_on_their_volume() {
        // These are rejected before touching the filesystem.
        let m = Path::new("/media");
        assert!(resolve(m, "Nope/x", &vols(), false).is_err());
        assert!(resolve(m, "Stick/../../etc/passwd", &vols(), false).is_err());
        assert!(resolve(m, "Stick/a/\0b", &vols(), false).is_err());
        assert!(resolve(m, "Locked/x", &vols(), true).is_err());
    }
    #[test]
    fn a_symlink_off_the_drive_is_refused() {
        let media = std::env::temp_dir().join(format!("ohc-files-test-{}", std::process::id()));
        let stick = media.join("Stick");
        std::fs::create_dir_all(stick.join("Music")).unwrap();
        std::os::unix::fs::symlink("/etc", stick.join("escape")).unwrap();
        std::os::unix::fs::symlink("Music", stick.join("inside")).unwrap();
        let v = vols();
        assert!(resolve(&media, "Stick/escape/passwd", &v, false).is_err());
        assert!(resolve(&media, "Stick/escape", &v, false).is_err());
        let (p, vol) = resolve(&media, "Stick/inside/new.mp3", &v, true).ok().unwrap();
        assert_eq!((p, vol.as_str()), (stick.join("inside/new.mp3"), "Stick"));
        assert!(resolve(&media, "Stick/Music/new folder", &v, true).is_ok());
        std::fs::remove_dir_all(&media).unwrap();
    }
    #[test]
    fn names() {
        assert!(valid_name("Song.mp3") && valid_name("a b"));
        assert!(!valid_name("") && !valid_name("..") && !valid_name("a/b"));
    }
    #[test]
    fn percent_encoding_for_downloads() {
        assert_eq!(pct("a b.mp3"), "a%20b.mp3");
        assert_eq!(pct("é"), "%C3%A9");
    }
}
