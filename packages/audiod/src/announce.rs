//! Announcements: a voice/chime channel mixed over whatever an output plays.
//!
//! What the EA-3 does in its DSP, done with ALSA: an announcement plays to
//! `<pcm>_announce`, which joins the output's dmix after the duck stage but
//! before the level (see levels.rs) — so the music under it is lowered
//! (`OHC_AUDIO_DUCK_DB`, default 20 dB), the announcement is not, and both
//! follow the output's volume. The duck ramps down before and back up after,
//! so nothing clicks.
//!
//! `cmd/audio/announce/<output|all>` with a payload of `chime` (or empty) for
//! the built-in two-tone chime, an http(s) URL, or an absolute path, to a WAV
//! (aplay plays it; the plug in front of the dmix converts rate and format).
//! One announcement at a time per output; more queue.
use crate::board::Port;
use crate::levels::{duck_control, duck_raw, DUCK_MAX};
use std::time::Duration;
use tokio::process::Command;

pub const CHIME: &str = "/run/ohc/audio/chime.wav";
/// Longest an announcement may play before it is cut off.
const MAX_SECS: u64 = 300;

pub fn duck_db() -> f64 {
    std::env::var("OHC_AUDIO_DUCK_DB").ok().and_then(|v| v.parse().ok()).unwrap_or(20.0)
}

async fn cset(p: &Port, control: &str, raw: u32) {
    let _ = Command::new("amixer")
        .args(["-q", "-c", &p.card(), "cset", &format!("name={control}"), &raw.to_string()])
        .status()
        .await;
}

/// Move the duck stage from `from` to `to` over `ms`.
async fn ramp(p: &Port, from: u32, to: u32, ms: u64) {
    const STEPS: u32 = 8;
    let ctl = duck_control(p);
    for i in 1..=STEPS {
        let v = (i64::from(from) + (i64::from(to) - i64::from(from)) * i64::from(i) / i64::from(STEPS)) as u32;
        cset(p, &ctl, v).await;
        tokio::time::sleep(Duration::from_millis(ms / u64::from(STEPS))).await;
    }
}

/// Where the announcement's audio is, fetching a URL first.
async fn resolve(source: &str, out_id: &str) -> Result<String, String> {
    let s = source.trim();
    if s.is_empty() || s == "chime" {
        return Ok(CHIME.into());
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        let file = format!("/run/ohc/audio/announce-{out_id}.wav");
        let st = Command::new("wget").args(["-q", "-T", "15", "-O", &file, s]).status().await;
        return match st {
            Ok(st) if st.success() => Ok(file),
            other => Err(format!("cannot fetch {s} ({other:?})")),
        };
    }
    if s.starts_with('/') && std::path::Path::new(s).is_file() {
        return Ok(s.into());
    }
    Err(format!("'{s}' is not 'chime', an http(s) URL or an existing absolute path"))
}

/// Play one announcement on `p`, ducking its music around it.
pub async fn play(p: &Port, source: &str) -> Result<(), String> {
    let file = resolve(source, &p.id).await?;
    let low = duck_raw(duck_db());
    ramp(p, DUCK_MAX, low, 240).await;
    let mut child = Command::new("aplay")
        .args(["-q", "-D", &format!("{}_announce", p.pcm), &file])
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("aplay: {e}"))?;
    let r = match tokio::time::timeout(Duration::from_secs(MAX_SECS), child.wait()).await {
        Ok(Ok(st)) if st.success() => Ok(()),
        Ok(other) => Err(format!("aplay {file}: {other:?}")),
        Err(_) => Err(format!("{file}: longer than {MAX_SECS} s, cut off")),
    };
    drop(child);
    ramp(p, low, DUCK_MAX, 600).await;
    r
}

/// The built-in chime: a two-note "ding-dong" (E5 then C5), bell-like (a soft
/// octave partial, exponential decay), peaking around −6 dBFS. 16-bit stereo
/// WAV at `rate`.
pub fn chime_wav(rate: u32) -> Vec<u8> {
    let r = f64::from(rate);
    let len = (1.6 * r) as usize;
    let note = |t: f64, start: f64, f: f64| -> f64 {
        let t = t - start;
        if t < 0.0 {
            return 0.0;
        }
        let env = (t / 0.004).min(1.0) * (-t / 0.38).exp();
        let w = std::f64::consts::TAU * f * t;
        env * (w.sin() + 0.3 * (2.0 * w).sin() + 0.08 * (3.0 * w).sin())
    };
    let mut pcm = Vec::with_capacity(len * 4);
    for i in 0..len {
        let t = i as f64 / r;
        let v = 0.36 * (note(t, 0.0, 659.25) + note(t, 0.5, 523.25));
        let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // channels
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chime_is_a_valid_wav_without_clipping() {
        let w = chime_wav(44100);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        let data = u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize;
        assert_eq!(w.len(), 44 + data);
        let peak = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
        assert!(peak > 8000 && peak < 32767, "peak {peak}");
    }
}
