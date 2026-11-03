//! The IO microcontroller's half of a return to stock: put Control4's own
//! application back on it, so the box Control4 gets back looks like openHC never
//! ran there. The bootloader in the part's low 4 KB is never touched.
//!
//! Control4 keeps the images in `/control4/firmware/io/` of the stock rootfs,
//! with `.flash.config` saying which one each device type runs; this reads them
//! from there (p1 just reimaged on an EA, the recovery archive on an HC-800).
//! The image is wrapped in openHC's header and written through the driver's
//! firmware-upload device, `/sys/class/firmware/ohc-iomcu`, which drives the
//! part's TI serial bootloader. See board/common/firmware/io-mcu/README.md.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const UPLOAD: &str = "/sys/class/firmware/ohc-iomcu";
const INFO: &str = "/sys/class/ohc/iomcu";
const WORK: &str = "/run/ohc-iomcu-stock";
const ARCHIVE_DIR: &str = "./control4/firmware/io";

const APP_BASE: usize = 0x1000;
const FLASH_SIZE: usize = 0x10000;
const C4_HDR: usize = 0x100;

/// Put Control4's firmware back from a stock rootfs tree (`root/control4/firmware/io`).
/// Ok carries what happened; nothing to do is Ok too.
pub fn restore_from_root(root: &Path) -> Result<String, String> {
    let Some(part) = present()? else {
        return Ok("no IO microcontroller here".into());
    };
    let running = running();
    if !running.starts_with("openhc-") && !running.is_empty() {
        return Ok(format!(
            "the IO microcontroller already runs Control4's {running}"
        ));
    }
    let dir = root.join("control4/firmware/io");
    let (file, data) = pick(&dir, &device_type()?, &part)?;
    let (img, version) = wrap(&data, &part, &file)?;
    println!(
        "iomcu: writing Control4's {version} ({file}) over {}",
        if running.is_empty() {
            "no running application"
        } else {
            &running
        }
    );
    upload(&img)?;
    let now = running_after(Duration::from_secs(5));
    if now != version {
        return Err(format!(
            "wrote Control4's {version}, but the part says {now:?}"
        ));
    }
    Ok(format!(
        "the IO microcontroller runs Control4's {version} again"
    ))
}

/// Same, from the board's recovery archive (`/dev/<partition>:/<archive>`).
pub fn restore_from_archive(spec: &str) -> Result<String, String> {
    if present()?.is_none() {
        return Ok("no IO microcontroller here".into());
    }
    let (dev, path) = spec
        .split_once(':')
        .filter(|(d, p)| d.starts_with("/dev/") && p.starts_with('/'))
        .ok_or_else(|| format!("{spec:?}: expected /dev/<partition>:/<archive>"))?;
    let dec = if path.ends_with(".tar.xz") {
        "unxz -c"
    } else if path.ends_with(".tgz") || path.ends_with(".tar.gz") {
        "gunzip -c"
    } else {
        return Err(format!("{path}: not a .tgz or .tar.xz"));
    };
    if [dev, path].iter().any(|s| s.contains('\'')) {
        return Err("a quote in the archive name; not passing it through a shell".into());
    }
    let mnt = Path::new(WORK).join("mnt");
    let out = Path::new(WORK).join("x");
    let _ = std::fs::remove_dir_all(WORK);
    std::fs::create_dir_all(&mnt)
        .and_then(|_| std::fs::create_dir_all(&out))
        .map_err(|e| format!("{WORK}: {e}"))?;
    if !Command::new("mount")
        .args(["-o", "ro", dev])
        .arg(&mnt)
        .status()
        .is_ok_and(|s| s.success())
    {
        return Err(format!("cannot mount {dev} read-only"));
    }
    // busybox tar exits 1 for whichever spelling of the member did not occur
    let sh = format!(
        "{dec} '{}' | tar -x -f - -C '{}' '{ARCHIVE_DIR}' '{}' 2>/dev/null; true",
        mnt.join(path.trim_start_matches('/')).display(),
        out.display(),
        ARCHIVE_DIR.trim_start_matches("./"),
    );
    let _ = Command::new("sh").args(["-c", &sh]).status();
    let _ = Command::new("umount").arg(&mnt).status();
    let r = restore_from_root(&out);
    let _ = std::fs::remove_dir_all(WORK);
    r
}

/// The part, when this kernel has the driver and its loader; None when not.
fn present() -> Result<Option<String>, String> {
    if !Path::new(UPLOAD).exists() {
        return Ok(None);
    }
    let part = std::fs::read_to_string(format!("{INFO}/part"))
        .unwrap_or_default()
        .trim()
        .to_string();
    if part.is_empty() {
        return Err(format!("{UPLOAD} exists but {INFO}/part names no part"));
    }
    Ok(Some(part))
}

/// What the part answered FIRMWARE_VERSION with, once it has (the driver
/// identifies it a second or so after boot).
fn running() -> String {
    running_after(Duration::from_secs(5))
}

fn running_after(wait: Duration) -> String {
    let t = Instant::now();
    loop {
        let v = std::fs::read_to_string(format!("{INFO}/firmware"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !v.is_empty() || t.elapsed() >= wait {
            return v;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Control4's name for this board in `.flash.config`.
fn device_type() -> Result<String, String> {
    if crate::hc800::is_hc800() {
        return Ok("hc800".into());
    }
    let c = std::fs::read("/proc/device-tree/compatible")
        .map_err(|e| format!("/proc/device-tree/compatible: {e}"))?;
    let first =
        String::from_utf8_lossy(c.split(|&b| b == 0).next().unwrap_or_default()).to_string();
    device_type_of(&first).ok_or_else(|| format!("no Control4 device type in compatible {first:?}"))
}

/// "control4,ea3-v1" -> "ea3"
fn device_type_of(compatible: &str) -> Option<String> {
    let model = compatible.strip_prefix("control4,")?;
    Some(model.split('-').next()?.to_string()).filter(|m| !m.is_empty())
}

/// The application `.flash.config` lists for `device`, or the only one for the part.
fn pick(dir: &Path, device: &str, part: &str) -> Result<(String, Vec<u8>), String> {
    let file = match std::fs::read_to_string(dir.join(".flash.config")) {
        Ok(cfg) => application_for(&cfg, device).ok_or_else(|| {
            format!(
                "{}: no application for {device}",
                dir.join(".flash.config").display()
            )
        })?,
        Err(_) => {
            let tag = part.to_uppercase();
            let mut apps: Vec<PathBuf> = std::fs::read_dir(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().unwrap_or_default().to_string_lossy();
                    n.contains(&tag) && !n.contains("Bootloader")
                })
                .collect();
            apps.sort();
            match apps.as_slice() {
                [one] => one
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                _ => {
                    return Err(format!(
                        "{}: no .flash.config and {} candidate images",
                        dir.display(),
                        apps.len()
                    ))
                }
            }
        }
    };
    let data = std::fs::read(dir.join(&file))
        .map_err(|e| format!("{}: {e}", dir.join(&file).display()))?;
    Ok((file, data))
}

/// `<Device type="ea3"> ... <application>NAME</application>` -> NAME
fn application_for(cfg: &str, device: &str) -> Option<String> {
    let start = cfg.find(&format!("type=\"{device}\""))?;
    let rest = &cfg[start..];
    let end = rest.find("</Device>").unwrap_or(rest.len());
    let block = &rest[..end];
    let a = block.find("<application>")? + "<application>".len();
    let b = block[a..].find("</application>")? + a;
    Some(block[a..b].trim().to_string())
}

/// The app-slot bytes of a Control4 image in openHC's header, and its version
/// (what the part will answer FIRMWARE_VERSION with). Same layout as
/// board/common/firmware/io-mcu/tools/mkfw.py.
fn wrap(data: &[u8], part: &str, file: &str) -> Result<(Vec<u8>, String), String> {
    let (flash, version, id) = match part {
        "lm3s1162" => {
            if data.len() != C4_HDR + FLASH_SIZE + 2 {
                return Err(format!(
                    "{file}: not a Control4 LM3S image ({} bytes)",
                    data.len()
                ));
            }
            let flash = &data[C4_HDR..C4_HDR + FLASH_SIZE];
            let crc =
                u16::from_le_bytes([data[C4_HDR + FLASH_SIZE], data[C4_HDR + FLASH_SIZE + 1]]);
            if crc16_arc(flash) != crc {
                return Err(format!("{file}: its CRC-16 does not match"));
            }
            (
                flash,
                header_version(&data[..C4_HDR]).ok_or_else(|| format!("{file}: no FWVERS"))?,
                2u32,
            )
        }
        "tm4c1231d5" => {
            if data.len() != FLASH_SIZE {
                return Err(format!(
                    "{file}: not a 64 KB TM4C image ({} bytes)",
                    data.len()
                ));
            }
            (
                data,
                name_version(file).ok_or_else(|| format!("{file}: no version in the name"))?,
                1u32,
            )
        }
        _ => return Err(format!("unknown part {part:?}")),
    };
    let app = &flash[APP_BASE..];
    let mut v = [0u8; 32];
    if version.len() >= v.len() {
        return Err(format!("version {version:?} too long"));
    }
    v[..version.len()].copy_from_slice(version.as_bytes());
    let mut h = Vec::with_capacity(64 + app.len());
    h.extend_from_slice(b"OHCIOMCU");
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&64u16.to_le_bytes());
    h.extend_from_slice(&id.to_le_bytes());
    h.extend_from_slice(&(APP_BASE as u32).to_le_bytes());
    h.extend_from_slice(&(app.len() as u32).to_le_bytes());
    h.extend_from_slice(&crc32(app).to_le_bytes());
    h.extend_from_slice(&v);
    let hc = crc32(&h);
    h.extend_from_slice(&hc.to_le_bytes());
    h.extend_from_slice(app);
    Ok((h, version))
}

/// "FWVERS:\0" "03.26.15" "\0" in the LM3S container's text header.
fn header_version(hdr: &[u8]) -> Option<String> {
    let i = hdr.windows(7).position(|w| w == b"FWVERS:")? + 8;
    let v: Vec<u8> = hdr
        .get(i..)?
        .iter()
        .take_while(|&&b| b != 0)
        .copied()
        .collect();
    String::from_utf8(v).ok().filter(|s| !s.is_empty())
}

/// "..._Release_1.0.36.20ebb2d.bin" -> "1.0.36"
fn name_version(file: &str) -> Option<String> {
    let rest = &file[file.find("Release_")? + "Release_".len()..];
    let parts: Vec<&str> = rest.split('.').take(3).collect();
    (parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    .then(|| parts.join("."))
}

fn crc16_arc(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The firmware-upload ABI: loading=1, data, loading=0, then status back to idle.
fn upload(img: &[u8]) -> Result<(), String> {
    let w = |f: &str, v: &[u8]| -> Result<(), String> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(format!("{UPLOAD}/{f}"))
            .and_then(|mut h| h.write_all(v))
            .map_err(|e| format!("{UPLOAD}/{f}: {e}"))
    };
    w("loading", b"1")?;
    if let Err(e) = w("data", img) {
        let _ = w("loading", b"-1");
        return Err(e);
    }
    w("loading", b"0")?;
    let t = Instant::now();
    loop {
        let st = std::fs::read_to_string(format!("{UPLOAD}/status")).unwrap_or_default();
        if st.trim() == "idle" {
            break;
        }
        if t.elapsed() > Duration::from_secs(180) {
            let _ = w("cancel", b"1");
            return Err("the write did not finish in 3 minutes; cancelled".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let err = std::fs::read_to_string(format!("{UPLOAD}/error")).unwrap_or_default();
    if err.trim().is_empty() {
        Ok(())
    } else {
        Err(format!("the driver could not write it: {}", err.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLASH_CONFIG: &str = r#"<firmware>
	<Device type="hc800">
		<bootloader>IRBootloaderSerialLM3S1162.bin</bootloader>
		<application>700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin</application>
	</Device>
	<Device type="ea3">
		<bootloader>660-00063_TM4C1231D5_IO_Processor_Bootloader_Release_1.1.9.b6a901f.bin</bootloader>
		<application>660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin</application>
	</Device>
	<Device type="amp1">
		<application>660-00063_TM4C1231D5_IO_Processor_Release_1.0.41.093e513.bin</application>
	</Device>
</firmware>"#;

    #[test]
    fn flash_config() {
        assert_eq!(
            application_for(FLASH_CONFIG, "ea3").as_deref(),
            Some("660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin")
        );
        assert_eq!(
            application_for(FLASH_CONFIG, "hc800").as_deref(),
            Some("700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin")
        );
        assert_eq!(application_for(FLASH_CONFIG, "ea5"), None);
    }

    #[test]
    fn device_types() {
        assert_eq!(device_type_of("control4,ea3-v1").as_deref(), Some("ea3"));
        assert_eq!(device_type_of("control4,ea5").as_deref(), Some("ea5"));
        assert_eq!(device_type_of("intel,ce5300"), None);
    }

    #[test]
    fn versions() {
        assert_eq!(
            name_version("660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin").as_deref(),
            Some("1.0.36")
        );
        assert_eq!(name_version("x.bin"), None);
        let mut hdr = vec![0xffu8; 256];
        let s = b"\x00\x0cIR_Processor\xff\xffFWVERS:\x0003.26.15\x00";
        hdr[..s.len()].copy_from_slice(s);
        assert_eq!(header_version(&hdr).as_deref(), Some("03.26.15"));
    }

    #[test]
    fn checksums() {
        assert_eq!(crc16_arc(b"123456789"), 0xBB3D);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn wraps_lm3s() {
        let mut flash = vec![0xffu8; FLASH_SIZE];
        flash[APP_BASE] = 0x42;
        let mut img = vec![0xffu8; C4_HDR];
        let s = b"FWVERS:\x0003.26.15\x00";
        img[16..16 + s.len()].copy_from_slice(s);
        img.extend_from_slice(&flash);
        img.extend_from_slice(&crc16_arc(&flash).to_le_bytes());
        let (out, v) = wrap(&img, "lm3s1162", "x.bin").unwrap();
        assert_eq!(v, "03.26.15");
        assert_eq!(&out[..8], b"OHCIOMCU");
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(out[16..20].try_into().unwrap()), 0x1000);
        assert_eq!(out.len(), 64 + FLASH_SIZE - APP_BASE);
        assert_eq!(out[64], 0x42);
        assert_eq!(
            u32::from_le_bytes(out[60..64].try_into().unwrap()),
            crc32(&out[..60])
        );
        assert_eq!(
            u32::from_le_bytes(out[24..28].try_into().unwrap()),
            crc32(&out[64..])
        );
        *img.last_mut().unwrap() ^= 1;
        assert!(wrap(&img, "lm3s1162", "x.bin").is_err());
    }

    #[test]
    fn wraps_tm4c() {
        let data = vec![0u8; FLASH_SIZE];
        let (out, v) = wrap(
            &data,
            "tm4c1231d5",
            "660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin",
        )
        .unwrap();
        assert_eq!(v, "1.0.36");
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 1);
        assert!(wrap(&data[1..], "tm4c1231d5", "x_Release_1.0.36.b.bin").is_err());
    }
}
