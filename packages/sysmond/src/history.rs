//! A fixed-size ring of samples.
//!
//! Bounded on purpose. This runs on a controller with 2 GB and a RAM rootfs, so
//! history has a hard ceiling rather than a policy: the ring overwrites, and the
//! daemon's memory is the same after a month as after a minute.
//!
//! Samples are stored COMPACT — bare numbers in a fixed order, with the sensor
//! labels held once alongside. A full Snapshot repeats every label string, and
//! at one sample per 5 s over six hours that is four thousand copies of the word
//! "CPUTIN". The API rehydrates on the way out.
//!
//! Downsampling is deliberately absent. One ring at one cadence is a shape
//! anybody can reason about; the moment there are 10-second and hourly buckets,
//! somebody has to decide which one a query means.
use serde::Serialize;
use std::collections::VecDeque;

/// What a sensor IS, stored once.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Series {
    pub slug: String,
    pub label: String,
    pub chip: String,
    pub kind: &'static str, // temp | fan | pwm
}

/// What every sensor READ at one instant. Positional against `Store::series`.
#[derive(Serialize, Clone, Debug)]
pub struct Sample {
    pub at: u64,
    pub v: Vec<Option<i64>>,
    pub cpu: Option<u8>,
    pub load1: Option<f32>,
    pub mem_used_pct: Option<u8>,
}

pub struct Store {
    pub series: Vec<Series>,
    ring: VecDeque<Sample>,
    cap: usize,
}

impl Store {
    /// `window` seconds of history at `period` seconds per sample.
    pub fn new(window: u64, period: u64) -> Self {
        let cap = (window / period.max(1)).max(2) as usize;
        Store { series: Vec::new(), ring: VecDeque::with_capacity(cap), cap }
    }

    /// Take a reading set and align it to the known series.
    ///
    /// A sensor that appears later (a USB part, a driver bound late) EXTENDS the
    /// series list rather than invalidating history: older samples simply have
    /// no value at that index, which is the truth. Renumbering would silently
    /// re-label every point already recorded.
    pub fn push(&mut self, at: u64, readings: &[(Series, i64)], cpu: Option<u8>,
                load1: Option<f32>, mem_used_pct: Option<u8>) {
        for (s, _) in readings {
            if !self.series.iter().any(|k| k.slug == s.slug && k.kind == s.kind) {
                self.series.push(s.clone());
            }
        }
        let v = self
            .series
            .iter()
            .map(|k| {
                readings
                    .iter()
                    .find(|(s, _)| s.slug == k.slug && s.kind == k.kind)
                    .map(|(_, val)| *val)
            })
            .collect();
        if self.ring.len() == self.cap {
            self.ring.pop_front();
        }
        self.ring.push_back(Sample { at, v, cpu, load1, mem_used_pct });
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.ring.back()
    }

    /// Everything at or after `since` unix seconds; all of it when `since` is 0.
    pub fn since(&self, since: u64) -> Vec<&Sample> {
        self.ring.iter().filter(|s| s.at >= since).collect()
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
}
