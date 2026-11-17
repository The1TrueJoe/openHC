//! Read single members of a remote zip with HTTP range requests, so a return
//! to stock fetches the few MB it needs from an image bundle instead of the
//! whole ~1 GB. Like the rest of the engine, this shells out to `curl`.

use std::io::{self, Read, Seek};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// `bytes 0-0/950353694` -> 950353694, from a ranged response's headers.
fn total_from_headers(headers: &str) -> Option<u64> {
    headers.lines().filter_map(|l| {
        let (k, v) = l.split_once(':')?;
        if !k.trim().eq_ignore_ascii_case("content-range") {
            return None;
        }
        v.rsplit('/').next()?.trim().parse().ok()
    }).next_back()
}

fn curl_range(url: &str, start: u64, end: u64, headers: bool) -> io::Result<Vec<u8>> {
    let mut c = Command::new("curl");
    c.args(["-sfL", "-A", "ohc-flasher", "-r", &format!("{start}-{end}")]);
    if headers {
        c.args(["-D", "-", "-o", "/dev/null"]);
    }
    let out = c.arg(url).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "curl range {start}-{end} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// The remote file's size, from a one-byte ranged GET (HEAD can fail on signed
/// CDN URLs).
fn remote_len(url: &str) -> Result<u64> {
    let h = curl_range(url, 0, 0, true).context("asking the server for the bundle's size")?;
    total_from_headers(&String::from_utf8_lossy(&h))
        .ok_or_else(|| anyhow!("the server does not support range requests for {url}"))
}

/// The member named `name` (at the top, or under one directory) of the zip at
/// `url`. Four requests: the tail (end of central directory), the central
/// directory, the member's local header, its data.
pub fn fetch_member(url: &str, name: &str) -> Result<Vec<u8>> {
    let len = remote_len(url)?;
    let tail_start = len.saturating_sub(66 * 1024);
    let tail = curl_range(url, tail_start, len - 1, false)?;
    let eocd = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or_else(|| anyhow!("no zip end-of-central-directory record"))?;
    let le32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as u64;
    let le16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let (cd_size, cd_off) = (le32(&tail, eocd + 12), le32(&tail, eocd + 16));
    if cd_off == 0xffff_ffff || cd_size == 0xffff_ffff {
        bail!("the bundle is a zip64 archive, which this reader does not handle");
    }
    let cd = curl_range(url, cd_off, cd_off + cd_size - 1, false)?;
    let mut o = 0;
    while o + 46 <= cd.len() && &cd[o..o + 4] == b"PK\x01\x02" {
        let (method, csize) = (le16(&cd, o + 10), le32(&cd, o + 20));
        let (nlen, elen, clen) = (le16(&cd, o + 28), le16(&cd, o + 30), le16(&cd, o + 32));
        let local = le32(&cd, o + 42);
        let n = String::from_utf8_lossy(&cd[o + 46..o + 46 + nlen]);
        if n == name || n.ends_with(&format!("/{name}")) {
            let lh = curl_range(url, local, local + 29, false)?;
            if &lh[..4] != b"PK\x03\x04" {
                bail!("{name}: bad local header");
            }
            let start = local + 30 + le16(&lh, 26) as u64 + le16(&lh, 28) as u64;
            let data = curl_range(url, start, start + csize - 1, false)?;
            return match method {
                0 => Ok(data),
                8 => {
                    let mut v = Vec::new();
                    flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut v)?;
                    Ok(v)
                }
                m => bail!("{name}: unsupported zip compression method {m}"),
            };
        }
        o += 46 + nlen + elen + clen;
    }
    bail!("the bundle has no {name}")
}

/// The member named `name` from any open zip.
pub fn member<R: Read + Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let idx = (0..z.len())
        .find(|&i| {
            z.name_for_index(i)
                .and_then(|r| r.ok())
                .is_some_and(|n| n == name || n.ends_with(&format!("/{name}")))
        })
        .ok_or_else(|| anyhow!("the bundle has no {name}"))?;
    let mut f = z.by_index(idx)?;
    let mut v = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut v)?;
    if v.is_empty() {
        bail!("{name} in the bundle is empty");
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_total() {
        let h = "HTTP/2 302\r\nlocation: x\r\n\r\nHTTP/2 206\r\nContent-Range: bytes 0-0/950353694\r\n";
        assert_eq!(total_from_headers(h), Some(950353694));
        assert_eq!(total_from_headers("HTTP/2 200\r\ncontent-length: 5\r\n"), None);
    }

    #[test]
    fn member_by_name_or_under_a_directory() {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("bundle/boot-init.cpio.gz", o).unwrap();
            std::io::Write::write_all(&mut w, b"init").unwrap();
            w.start_file("bzImage-real", o).unwrap();
            std::io::Write::write_all(&mut w, b"kern").unwrap();
            w.finish().unwrap();
        }
        let mut z = zip::ZipArchive::new(io::Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(member(&mut z, "boot-init.cpio.gz").unwrap(), b"init");
        assert_eq!(member(&mut z, "bzImage-real").unwrap(), b"kern");
        assert!(member(&mut z, "rootfs.ext2").is_err());
    }
}
