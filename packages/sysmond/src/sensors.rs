//! Reading the sensors: temperatures, fans, CPU, memory, uptime.
//!
//! All of it comes from files the kernel already publishes — `/sys/class/hwmon`
//! and `/proc` — so there is no polling of hardware here and nothing to own.
use serde::Serialize;

const HWMON: &str = "/sys/class/hwmon";

#[derive(Serialize, Clone, Debug)]
pub struct Reading {
    /// Topic slug: the sensor label, lowercased, or chip-qualified when two
    /// chips use the same label.
    pub slug: String,
    pub label: String,
    pub chip: String,
    /// Celsius for temps, RPM for fans, 0-255 for pwm.
    pub value: i64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Sensors {
    pub temps: Vec<Reading>,
    pub fans: Vec<Reading>,
    pub pwm: Vec<Reading>,
}

fn rd(p: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Every hwmon sensor on the box.
///
/// Labels are kept as the kernel gives them — CPUTIN and SYSTIN mean something
/// specific to whoever reads a datasheet, and renaming them to "cpu" and
/// "board" would be inventing precision we do not have about where the
/// thermistors actually sit.
pub fn sensors() -> Sensors {
    let mut out = Sensors::default();
    let Ok(dir) = std::fs::read_dir(HWMON) else {
        return out;
    };
    let mut chips: Vec<(String, std::path::PathBuf)> = dir
        .flatten()
        .filter_map(|e| rd(&e.path().join("name")).map(|n| (n, e.path())))
        .collect();
    chips.sort();

    for (chip, path) in &chips {
        for i in 1..=8 {
            if let Some(v) = rd(&path.join(format!("temp{i}_input"))).and_then(|s| s.parse::<i64>().ok()) {
                let label = rd(&path.join(format!("temp{i}_label"))).unwrap_or(format!("temp{i}"));
                out.temps.push(Reading { slug: slugify(&label), label, chip: chip.clone(), value: v / 1000 });
            }
            if let Some(v) = rd(&path.join(format!("fan{i}_input"))).and_then(|s| s.parse::<i64>().ok()) {
                let label = rd(&path.join(format!("fan{i}_label"))).unwrap_or(format!("fan{i}"));
                out.fans.push(Reading { slug: slugify(&label), label, chip: chip.clone(), value: v });
            }
            if let Some(v) = rd(&path.join(format!("pwm{i}"))).and_then(|s| s.parse::<i64>().ok()) {
                out.pwm.push(Reading { slug: format!("pwm{i}"), label: format!("pwm{i}"), chip: chip.clone(), value: v });
            }
        }
    }
    // Qualify only what collides, the same rule the LEDs use.
    for list in [&mut out.temps, &mut out.fans, &mut out.pwm] {
        let slugs: Vec<String> = list.iter().map(|r| r.slug.clone()).collect();
        for r in list.iter_mut() {
            if slugs.iter().filter(|s| **s == r.slug).count() > 1 {
                r.slug = format!("{}-{}", slugify(&r.chip), r.slug);
            }
        }
    }
    out
}

/// CPU busy percentage, from the delta between two /proc/stat samples.
///
/// Stateful by necessity: /proc/stat holds cumulative jiffies since boot, so a
/// single read tells you the average since power-on and nothing about now.
#[derive(Default)]
pub struct Cpu {
    prev: Option<(u64, u64)>,
}

impl Cpu {
    pub fn sample(&mut self) -> Option<u8> {
        let line = std::fs::read_to_string("/proc/stat").ok()?;
        let f: Vec<u64> = line
            .lines()
            .next()?
            .split_whitespace()
            .skip(1)
            .filter_map(|v| v.parse().ok())
            .collect();
        if f.len() < 4 {
            return None;
        }
        let total: u64 = f.iter().sum();
        let idle = f[3] + f.get(4).copied().unwrap_or(0); // idle + iowait
        let now = (total, idle);
        let out = self.prev.and_then(|(pt, pi)| {
            let dt = total.checked_sub(pt)?;
            let di = idle.checked_sub(pi)?;
            (dt > 0).then(|| (100 * (dt - di) / dt) as u8)
        });
        self.prev = Some(now);
        out
    }
}

/// `(total kB, available kB)`.
pub fn mem() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
    };
    Some((get("MemTotal:")?, get("MemAvailable:")?))
}

pub fn uptime_secs() -> Option<u64> {
    std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|f| f as u64)
}

pub fn load1() -> Option<f32> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Every reading, flattened for the history store.
pub fn readings() -> Vec<(crate::history::Series, i64)> {
    let s = sensors();
    let mk = |r: &Reading, kind: &'static str| {
        (
            crate::history::Series {
                slug: r.slug.clone(),
                label: r.label.clone(),
                chip: r.chip.clone(),
                kind,
            },
            r.value,
        )
    };
    s.temps
        .iter()
        .map(|r| mk(r, "temp"))
        .chain(s.fans.iter().map(|r| mk(r, "fan")))
        .chain(s.pwm.iter().map(|r| mk(r, "pwm")))
        .collect()
}
