// Embed the built React UI (ui/dist) into the binary as a table of
// (path, mime, etag, raw_bytes, gzip_bytes). If ui/dist is absent the table is
// empty and the server serves a tiny fallback page — so `cargo build` works
// without the UI, but a release overlay must build the UI first.
use std::io::Write;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=ui/dist");
    let dist = Path::new("ui/dist");
    let out = std::env::var("OUT_DIR").unwrap();
    let mut entries = String::from("pub static EMBEDDED_ASSETS: &[Asset] = &[\n");

    if dist.is_dir() {
        let mut files = Vec::new();
        walk(dist, &mut files);
        for path in files {
            let rel = path.strip_prefix(dist).unwrap().to_string_lossy().replace('\\', "/");
            let raw = std::fs::read(&path).unwrap();
            let mime = mime_for(&rel);
            let etag = format!("\"{:x}\"", fnv1a(&raw));
            // gzip; keep only if it saves >10%
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(&raw).unwrap();
            let gz = enc.finish().unwrap();
            let gz_lit = if gz.len() * 10 < raw.len() * 9 {
                format!("Some(&{:?})", gz)
            } else {
                "None".to_string()
            };
            entries.push_str(&format!(
                "  Asset {{ path: {:?}, mime: {:?}, etag: {:?}, raw: &{:?}, gzip: {} }},\n",
                rel, mime, etag, raw, gz_lit
            ));
        }
    }
    entries.push_str("];\n");
    std::fs::write(Path::new(&out).join("assets.rs"), entries).unwrap();
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); } else { out.push(p); }
    }
}

fn mime_for(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn fnv1a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b { h ^= x as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}
