//! Reading the sensors: temperatures, fans, CPU, memory, uptime.
//!
//! All of it comes from files the kernel already publishes — `/sys/class/hwmon`,
//! `/sys/class/thermal` and `/proc` — so there is no polling of hardware here
//! and nothing to own.
//!
//! A temperature the board's device tree puts in a THERMAL ZONE is read from
//! the zone, not from its sensor's hwmon channel: the zone's `temp` is the value
//! the kernel's governor acts on (the EA fan curve), so it is the one worth
//! plotting, and showing the raw channel as well would draw the same thermistor
//! twice. Zones that are not in the device tree (ACPI's `acpitz`) are skipped:
//! those register a hwmon of their own, which the hwmon walk already reads.
use serde::Serialize;
use std::path::{Path, PathBuf};

const HWMON: &str = "/sys/class/hwmon";
const THERMAL: &str = "/sys/class/thermal";
const DT_ZONES: &str = "/sys/firmware/devicetree/base/thermal-zones";

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

fn rd(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn num(p: &Path) -> Option<i64> {
    rd(p)?.parse().ok()
}

/// A device-tree property as big-endian cells.
fn cells(p: &Path) -> Vec<u32> {
    std::fs::read(p)
        .map(|b| b.chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect())
        .unwrap_or_default()
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// One hwmon device.
struct Chip {
    name: String,
    path: PathBuf,
    /// The chip-wide `label`, which the hwmon core publishes from the device's
    /// own `label` property (the board's device tree names its sensors there).
    label: Option<String>,
}

impl Chip {
    /// Channel indices (1-based) that have `<kind><i><suffix>`.
    fn channels(&self, kind: &str, suffix: &str) -> Vec<u32> {
        (1..=8).filter(|i| self.path.join(format!("{kind}{i}{suffix}")).exists()).collect()
    }

    /// A channel's own label, else the chip's label when this is the chip's
    /// only channel of that kind, else the bare channel name.
    fn label(&self, kind: &str, i: u32, only: bool) -> String {
        rd(&self.path.join(format!("{kind}{i}_label")))
            .or_else(|| self.label.clone().filter(|_| only))
            .unwrap_or(format!("{kind}{i}"))
    }
}

/// Every sensor on the box.
///
/// Labels are kept as the kernel gives them — CPUTIN and SYSTIN mean something
/// specific to whoever reads a datasheet, and renaming them to "cpu" and
/// "board" would be inventing precision we do not have about where the
/// thermistors actually sit.
pub fn sensors() -> Sensors {
    sensors_in(Path::new(HWMON), Path::new(THERMAL), Path::new(DT_ZONES))
}

fn sensors_in(hwmon: &Path, thermal: &Path, dt_zones: &Path) -> Sensors {
    let mut out = Sensors::default();
    let mut chips: Vec<Chip> = std::fs::read_dir(hwmon)
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let path = e.path();
                    Some(Chip { name: rd(&path.join("name"))?, label: rd(&path.join("label")), path })
                })
                .collect()
        })
        .unwrap_or_default();
    chips.sort_by(|a, b| (&a.name, &a.path).cmp(&(&b.name, &b.path)));

    // Device-tree thermal zones first: they are the temperatures the kernel
    // acts on. Each one hides the hwmon channel it reads.
    let mut covered: Vec<(PathBuf, u32)> = Vec::new();
    let mut zones: Vec<PathBuf> = std::fs::read_dir(thermal)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone")))
                .collect()
        })
        .unwrap_or_default();
    zones.sort();
    for z in &zones {
        let Some(kind) = rd(&z.join("type")) else { continue };
        let node = dt_zones.join(&kind);
        if !node.is_dir() {
            continue;
        }
        let Some(v) = num(&z.join("temp")) else { continue };
        // thermal-sensors = <&sensor [id]>: find the hwmon whose device-tree
        // node carries that phandle.
        let ts = cells(&node.join("thermal-sensors"));
        let sensor = ts.first().and_then(|ph| {
            chips.iter().find(|c| cells(&c.path.join("device/of_node/phandle")).first() == Some(ph))
        });
        if let Some(c) = sensor {
            let ncells = cells(&c.path.join("device/of_node/#thermal-sensor-cells")).first().copied().unwrap_or(0);
            let id = if ncells > 0 { ts.get(1).copied().unwrap_or(0) } else { 0 };
            covered.push((c.path.clone(), id + 1));
        }
        let label = sensor
            .and_then(|c| c.label.clone())
            .unwrap_or_else(|| kind.strip_suffix("-thermal").unwrap_or(&kind).to_string());
        let chip = sensor.map_or_else(|| "thermal".to_string(), |c| c.name.clone());
        out.temps.push(Reading { slug: slugify(&label), label, chip, value: v / 1000 });
    }

    for c in &chips {
        let temps = c.channels("temp", "_input");
        for &i in &temps {
            if covered.iter().any(|(p, n)| *p == c.path && *n == i) {
                continue;
            }
            if let Some(v) = num(&c.path.join(format!("temp{i}_input"))) {
                let label = c.label("temp", i, temps.len() == 1);
                out.temps.push(Reading { slug: slugify(&label), label, chip: c.name.clone(), value: v / 1000 });
            }
        }
        let fans = c.channels("fan", "_input");
        for &i in &fans {
            if let Some(v) = num(&c.path.join(format!("fan{i}_input"))) {
                let label = c.label("fan", i, fans.len() == 1);
                out.fans.push(Reading { slug: slugify(&label), label, chip: c.name.clone(), value: v });
            }
        }
        let pwms = c.channels("pwm", "");
        for &i in &pwms {
            if let Some(v) = num(&c.path.join(format!("pwm{i}"))) {
                let label = c.label("pwm", i, pwms.len() == 1);
                out.pwm.push(Reading { slug: slugify(&label), label, chip: c.name.clone(), value: v });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    /// The EA layout: an lm75 that a device-tree zone reads, and a pwm-fan,
    /// both named by their device-tree `label`s.
    #[test]
    fn dt_zone_replaces_its_sensor_channel() {
        let root = std::env::temp_dir().join(format!("sysmond-sensors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        put(&root, "hwmon/hwmon0/name", b"lm75\n");
        put(&root, "hwmon/hwmon0/label", b"Heatsink\n");
        put(&root, "hwmon/hwmon0/temp1_input", b"41500\n");
        put(&root, "hwmon/hwmon0/device/of_node/phandle", &7u32.to_be_bytes());
        put(&root, "hwmon/hwmon0/device/of_node/#thermal-sensor-cells", &0u32.to_be_bytes());
        put(&root, "hwmon/hwmon1/name", b"pwmfan\n");
        put(&root, "hwmon/hwmon1/label", b"Fan\n");
        put(&root, "hwmon/hwmon1/pwm1", b"77\n");
        put(&root, "hwmon/hwmon1/pwm1_enable", b"1\n");
        put(&root, "thermal/thermal_zone0/type", b"heatsink-thermal\n");
        put(&root, "thermal/thermal_zone0/temp", b"42000\n");
        put(&root, "thermal/thermal_zone1/type", b"acpitz\n");
        put(&root, "thermal/thermal_zone1/temp", b"30000\n");
        put(&root, "dt/heatsink-thermal/thermal-sensors", &7u32.to_be_bytes());

        let s = sensors_in(&root.join("hwmon"), &root.join("thermal"), &root.join("dt"));
        std::fs::remove_dir_all(&root).unwrap();

        let temps: Vec<_> = s.temps.iter().map(|r| (r.slug.as_str(), r.label.as_str(), r.chip.as_str(), r.value)).collect();
        assert_eq!(temps, [("heatsink", "Heatsink", "lm75", 42)]);
        let pwm: Vec<_> = s.pwm.iter().map(|r| (r.slug.as_str(), r.label.as_str(), r.chip.as_str(), r.value)).collect();
        assert_eq!(pwm, [("fan", "Fan", "pwmfan", 77)]);
    }

    /// No device tree (the HC-800's coretemp, an ACPI zone's own hwmon): the
    /// hwmon walk is unchanged.
    #[test]
    fn plain_hwmon_without_zones() {
        let root = std::env::temp_dir().join(format!("sysmond-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        put(&root, "hwmon/hwmon0/name", b"coretemp\n");
        put(&root, "hwmon/hwmon0/temp2_input", b"50000\n");
        put(&root, "hwmon/hwmon0/temp2_label", b"Core 0\n");
        put(&root, "hwmon/hwmon0/temp3_input", b"51000\n");
        put(&root, "hwmon/hwmon0/temp3_label", b"Core 1\n");
        put(&root, "hwmon/hwmon1/name", b"acpitz\n");
        put(&root, "hwmon/hwmon1/temp1_input", b"30000\n");
        put(&root, "thermal/thermal_zone0/type", b"acpitz\n");
        put(&root, "thermal/thermal_zone0/temp", b"30000\n");

        let s = sensors_in(&root.join("hwmon"), &root.join("thermal"), &root.join("dt"));
        std::fs::remove_dir_all(&root).unwrap();

        let temps: Vec<_> = s.temps.iter().map(|r| (r.slug.as_str(), r.chip.as_str(), r.value)).collect();
        assert_eq!(temps, [("temp1", "acpitz", 30), ("core-0", "coretemp", 50), ("core-1", "coretemp", 51)]);
    }
}
