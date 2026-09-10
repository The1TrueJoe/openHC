//! Front-panel LEDs, through the kernel's LED class.
//!
//! `/sys/class/leds/<name>/brightness` — no chardev, no ownership to negotiate,
//! and the same interface every Linux tool already uses. Nothing here holds
//! anything open, for the same reason [`crate::gpio_io`] does not hold GPIO
//! lines: `echo 1 > /sys/class/leds/.../brightness` has to keep working.
//!
//! Names follow the LED class convention `<device>:<colour>:<function>`, which
//! is what the kernel driver registers. The topic slug drops the device and
//! keeps the colour only when it is needed to disambiguate — so an HC-800 gets
//! `wifi-red`, `wifi-yellow`, `wifi-blue`, `data`, `network`, `power` rather
//! than six names that all start with the same word.
use std::io;
use std::path::PathBuf;

const CLASS: &str = "/sys/class/leds";

#[derive(Clone, Debug, serde::Serialize)]
pub struct Led {
    /// Topic slug: what MQTT and the GUI use.
    pub slug: String,
    /// The kernel's name, for anyone going straight to sysfs.
    pub name: String,
    pub colour: String,
    pub function: String,
    pub max: u32,
    /// The kernel trigger driving it, if any. `none` means software control.
    pub trigger: String,
}

fn read(p: &PathBuf) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// The active trigger is the one in brackets: `none timer [heartbeat] ...`.
fn active_trigger(s: &str) -> String {
    s.split_whitespace()
        .find_map(|t| t.strip_prefix('[').and_then(|t| t.strip_suffix(']')))
        .unwrap_or("none")
        .to_string()
}

/// Every LED this board exposes.
pub fn list() -> Vec<Led> {
    let Ok(dir) = std::fs::read_dir(CLASS) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String, String, u32, String)> = Vec::new();
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // <device>:<colour>:<function>. Anything else is not ours to guess at.
        let parts: Vec<&str> = name.split(':').collect();
        let (colour, function) = match parts.len() {
            3 => (parts[1].to_string(), parts[2].to_string()),
            _ => (String::new(), name.clone()),
        };
        let max = read(&e.path().join("max_brightness"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let trig = read(&e.path().join("trigger"))
            .map(|s| active_trigger(&s))
            .unwrap_or_else(|| "none".into());
        found.push((name, colour, function, max, trig));
    }
    // Keep the colour only where the function alone would collide.
    let mut out: Vec<Led> = found
        .iter()
        .map(|(name, colour, function, max, trig)| {
            let dupes = found.iter().filter(|f| &f.2 == function).count();
            let slug = if dupes > 1 && !colour.is_empty() {
                format!("{function}-{colour}")
            } else {
                function.clone()
            };
            Led {
                slug,
                name: name.clone(),
                colour: colour.clone(),
                function: function.clone(),
                max: *max,
                trigger: trig.clone(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

fn by_slug(slug: &str) -> io::Result<Led> {
    list()
        .into_iter()
        .find(|l| l.slug == slug)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no LED {slug:?}")))
}

pub fn get(slug: &str) -> io::Result<u32> {
    let led = by_slug(slug)?;
    read(&PathBuf::from(CLASS).join(&led.name).join("brightness"))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("cannot read brightness"))
}

/// Set brightness. `None` means full, which is what a plain on/off caller wants
/// without having to know max_brightness.
pub fn set(slug: &str, level: Option<u32>) -> io::Result<u32> {
    let led = by_slug(slug)?;
    let want = level.unwrap_or(led.max).min(led.max);
    // A LED under a kernel trigger ignores writes, and silently doing nothing is
    // the worst answer. Drop the trigger first: software control is what a
    // caller writing brightness is asking for.
    if led.trigger != "none" {
        let _ = std::fs::write(PathBuf::from(CLASS).join(&led.name).join("trigger"), "none");
    }
    std::fs::write(
        PathBuf::from(CLASS).join(&led.name).join("brightness"),
        want.to_string(),
    )?;
    get(slug)
}
