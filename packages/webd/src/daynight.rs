//! Auto day/night for the on-screen view: dark from sunset to sunrise.
//!
//! The display's theme is "light", "dark" or "auto". The first two are applied
//! as they are. "auto" is resolved here: the sun's times at the box's location
//! decide, and the result is pushed to the running view whenever it flips.
//!
//! Location: explicit latitude/longitude when display.conf has them, else the
//! principal location of the configured time zone (IANA zone.tab, bundled — the
//! HC-800 image ships no zoneinfo at all). A zone's reference city is usually
//! within half an hour of local sunrise/sunset; coordinates make it exact.
//!
//! The clock is UTC throughout: the sun's times are absolute instants, so the
//! box needs no local time zone for this (NTP keeps the clock itself right).
//!
//! Two outputs, both consumed by S95ohc-display:
//!   /run/ohc-display.scheme  the scheme in force now ("light"/"dark"). The
//!                            service reads it when it starts or reloads the
//!                            view in "auto", so the view never opens light and
//!                            flips a minute later.
//!   the control FIFO         `theme light|dark` to the running view when the
//!                            scheme flips (the same channel iod's cmd/display/*
//!                            uses; a view that is not running is skipped).

use crate::{api, Config};
use chrono::{DateTime, Days, Utc};
use std::sync::Arc;
use sunrise::{Coordinates, SolarDay, SolarEvent};

const ZONE_TAB: &str = include_str!("tz/zone.tab");
const BACKWARD: &str = include_str!("tz/backward");
const SCHEME_FILE: &str = "/run/ohc-display.scheme";

/// zone.tab / zone1970.tab coordinates: ±DDMM±DDDMM or ±DDMMSS±DDDMMSS (ISO 6709).
fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let split = s[1..].find(['+', '-'])? + 1;
    let (lat, lon) = s.split_at(split);
    let part = |p: &str, deg_digits: usize| -> Option<f64> {
        let sign = if p.starts_with('-') { -1.0 } else { 1.0 };
        let d = &p[1..];
        let deg: f64 = d.get(..deg_digits)?.parse().ok()?;
        let min: f64 = d.get(deg_digits..deg_digits + 2)?.parse().ok()?;
        let sec: f64 = match d.get(deg_digits + 2..) {
            Some("") | None => 0.0,
            Some(x) => x.parse().ok()?,
        };
        Some(sign * (deg + min / 60.0 + sec / 3600.0))
    };
    Some((part(lat, 2)?, part(lon, 3)?))
}

/// The principal location of an IANA time zone, following legacy names
/// (`US/Eastern` -> `America/New_York`) through tzdata's `backward` links.
pub fn zone_coordinates(zone: &str) -> Option<(f64, f64)> {
    let zone = zone.trim();
    let lookup = |z: &str| {
        ZONE_TAB.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
            let mut f = l.split('\t');
            let (_cc, coord, name) = (f.next()?, f.next()?, f.next()?);
            (name == z).then(|| parse_iso6709(coord)).flatten()
        })
    };
    lookup(zone).or_else(|| {
        BACKWARD.lines().find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            match f.as_slice() {
                ["Link", target, name, ..] if *name == zone => lookup(target),
                _ => None,
            }
        })
    })
}

/// Where the sun is reckoned for this display: explicit coordinates win.
pub fn location(d: &api::Display) -> Option<(f64, f64)> {
    match (d.latitude, d.longitude) {
        (Some(lat), Some(lon)) => Some((lat, lon)),
        _ => zone_coordinates(&d.timezone),
    }
}

/// Is it dark at `at`, and when does that next change?
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sun {
    pub dark: bool,
    pub next: Option<DateTime<Utc>>,
}

/// Sunrises and sunsets for the UTC days around `at`, in order. Three days so a
/// location's local evening (already "tomorrow" in UTC) or morning is covered;
/// a polar day or night just has no events, and the state holds.
pub fn sun(lat: f64, lon: f64, at: DateTime<Utc>) -> Option<Sun> {
    let coord = Coordinates::new(lat, lon)?;
    let today = at.date_naive();
    let mut events: Vec<(DateTime<Utc>, bool)> = Vec::new(); // (when, dark after)
    for day in [today.checked_sub_days(Days::new(1))?, today, today.checked_add_days(Days::new(1))?] {
        let solar = SolarDay::new(coord, day);
        if let Some(t) = solar.event_time(SolarEvent::Sunrise) {
            events.push((t, false));
        }
        if let Some(t) = solar.event_time(SolarEvent::Sunset) {
            events.push((t, true));
        }
    }
    events.sort_by_key(|e| e.0);
    let dark = events.iter().rev().find(|e| e.0 <= at).map(|e| e.1)?;
    let next = events.iter().find(|e| e.0 > at).map(|e| e.0);
    Some(Sun { dark, next })
}

/// The scheme in force for this display right now: its own theme, or for
/// "auto" the sun's verdict ("light" when the location is unknown).
pub fn scheme(d: &api::Display, at: DateTime<Utc>) -> &'static str {
    match d.theme.as_str() {
        "light" => "light",
        "dark" => "dark",
        _ => match location(d).and_then(|(lat, lon)| sun(lat, lon, at)) {
            Some(s) if s.dark => "dark",
            _ => "light",
        },
    }
}

/// Resolve now and hand the result on when it changed: the scheme file, and in
/// "auto" the running view. Called every minute, and by POST /api/display before
/// it reloads the service (which reads the file for the view it starts or tells).
pub fn apply(cfg: &Config) {
    let d = api::read_display(cfg);
    let now = scheme(&d, Utc::now());
    let was = std::fs::read_to_string(SCHEME_FILE).unwrap_or_default();
    if was.trim() == now {
        return;
    }
    if let Err(e) = std::fs::write(SCHEME_FILE, format!("{now}\n")) {
        eprintln!("webd: cannot write {SCHEME_FILE}: {e}");
    }
    if d.enabled && d.theme == "auto" {
        send_theme(now);
    }
}

/// `theme <scheme>` into the view's control FIFO, never blocking: opened
/// non-blocking write-only, which fails at once (ENXIO) when no view holds it,
/// and a missing FIFO means no view at all. Either way there is nothing to tell.
fn send_theme(scheme: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::var("OHC_DISPLAY_CTL").unwrap_or_else(|_| "/run/ohc-display.ctl".into());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
    {
        let _ = f.write_all(format!("theme {scheme}\n").as_bytes());
    }
}

/// Re-check once a minute: a sunset lands within a minute of the real thing.
pub fn spawn(cfg: Arc<Config>) {
    tokio::spawn(async move {
        loop {
            apply(&cfg);
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn iso6709_both_precisions() {
        let (lat, lon) = parse_iso6709("+404251-0740023").unwrap();
        assert!((lat - 40.714).abs() < 0.001 && (lon + 74.006).abs() < 0.001);
        let (lat, lon) = parse_iso6709("+2232+08822").unwrap();
        assert!((lat - 22.533).abs() < 0.001 && (lon - 88.367).abs() < 0.001);
    }

    #[test]
    fn zones_and_legacy_names() {
        assert!(zone_coordinates("America/New_York").is_some());
        assert_eq!(zone_coordinates("US/Eastern"), zone_coordinates("America/New_York"));
        assert_eq!(zone_coordinates("Asia/Calcutta"), zone_coordinates("Asia/Kolkata"));
        assert!(zone_coordinates("Europe/Oslo").is_some());
        assert!(zone_coordinates("Nowhere/Special").is_none());
        assert!(zone_coordinates("").is_none());
    }

    #[test]
    fn new_york_day_and_night() {
        let (lat, lon) = zone_coordinates("America/New_York").unwrap();
        // 2026-10-17: sunrise ~11:13 UTC (07:13 EDT), sunset ~22:15 UTC (18:15 EDT).
        let noon = Utc.with_ymd_and_hms(2026, 10, 17, 16, 0, 0).unwrap();
        let s = sun(lat, lon, noon).unwrap();
        assert!(!s.dark);
        let next = s.next.unwrap();
        assert!(next > Utc.with_ymd_and_hms(2026, 10, 17, 21, 45, 0).unwrap());
        assert!(next < Utc.with_ymd_and_hms(2026, 10, 17, 22, 45, 0).unwrap());
        // 20:00 EDT is already the next UTC day: still dark.
        let evening = Utc.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap();
        assert!(sun(lat, lon, evening).unwrap().dark);
        let early = Utc.with_ymd_and_hms(2026, 10, 17, 9, 0, 0).unwrap();
        assert!(sun(lat, lon, early).unwrap().dark);
    }

    #[test]
    fn scheme_honours_explicit_theme_and_unknown_location() {
        let at = Utc.with_ymd_and_hms(2026, 10, 18, 2, 0, 0).unwrap(); // NY night
        let mut d = api::Display::default();
        d.theme = "light".into();
        assert_eq!(scheme(&d, at), "light");
        d.theme = "auto".into();
        assert_eq!(scheme(&d, at), "light"); // no zone, no coordinates
        d.timezone = "America/New_York".into();
        assert_eq!(scheme(&d, at), "dark");
        d.latitude = Some(-33.87); // Sydney coordinates win over the zone: daytime there
        d.longitude = Some(151.21);
        assert_eq!(scheme(&d, at), "light");
    }
}
