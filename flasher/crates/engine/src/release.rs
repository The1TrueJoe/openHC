//! Load an openHC release from a folder or a `.zip`.
//!
//! A release is the set of files `make image` produces: `bzImage`,
//! `rootfs.cpio.gz`, `rootfs.ext2` for the EA boards; `zImage` + DTB +
//! `boot.scr` for the CA-1. The GUI lets the user drag either a folder or a
//! zip; both resolve to the same [`Release`] so the flows do not care which.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// A loaded release: named artefacts held in memory. These are small enough
/// (the largest, rootfs.ext2, is streamed straight to the box) that holding the
/// bytes is simpler than juggling file handles, and it makes zip and folder
/// sources identical.
pub struct Release {
    files: HashMap<String, Vec<u8>>,
    pub source: String,
}

impl Release {
    /// From a directory of files or a `.zip`, chosen by what the path is.
    pub fn open(path: &Path) -> Result<Release> {
        if path.is_dir() {
            Self::from_dir(path)
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            Self::from_zip(path)
        } else {
            bail!("{} is neither a folder nor a .zip release", path.display())
        }
    }

    fn from_dir(dir: &Path) -> Result<Release> {
        let mut files = HashMap::new();
        for name in KNOWN {
            let p = dir.join(name);
            if p.is_file() {
                files.insert((*name).to_string(), std::fs::read(&p)?);
            }
        }
        if files.is_empty() {
            bail!("no openHC images found in {}", dir.display());
        }
        Ok(Release { files, source: dir.display().to_string() })
    }

    fn from_zip(path: &Path) -> Result<Release> {
        let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(f).context("read zip")?;
        let mut files = HashMap::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if !entry.is_file() {
                continue;
            }
            // Match by basename, so a release zipped with or without a top
            // folder both work.
            let base = entry
                .name()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_string();
            if KNOWN.contains(&base.as_str()) {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf)?;
                files.insert(base, buf);
            }
        }
        if files.is_empty() {
            bail!("no openHC images inside {}", path.display());
        }
        Ok(Release { files, source: path.display().to_string() })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }

    pub fn has(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }
}

/// Artefact names the flasher knows. Anything else in the folder/zip is ignored.
pub const KNOWN: &[&str] = &[
    "bzImage",
    "rootfs.cpio.gz",
    "rootfs.ext2",
    "rootfs.ext2.gz",
    "boot-init.cpio.gz",
    "zImage",
    "openhc-ca1-zImage",
    "c4-imx6sl-ca1.dtb",
    "boot.scr",
];
