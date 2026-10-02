//! Web-controlled audio: ALSA outputs, the network receivers, and the one knob
//! that is genuinely ours to turn — the output selection and its volume.
//!
//! openHC's `audio` feature ships ALSA (alsa-lib + aplay/amixer/speaker-test),
//! **librespot** (Spotify Connect) and **shairport-sync** (AirPlay 1). Those two
//! are *receivers*: playback is driven from the phone, not from here. So this
//! module does NOT pretend to be a media player. It reports what is actually
//! true about the box and exposes only what the services actually support:
//!
//! * which ALSA outputs exist (discovered at runtime, never from board.env —
//!   HDMI audio on the other EA boards is future work, so "zero or more outputs"
//!   is the only honest model);
//! * which receivers are installed and running;
//! * which output is selected, and the volume on it (via `amixer`);
//! * now-playing metadata **iff** a receiver is actually feeding it to us.
//!
//! The governing rule is board.rs's: a thing with nothing behind it does not
//! appear. No sound card and no receiver binaries → [`capability`] returns
//! `None` and the whole section — REST, MQTT, the panel — is simply absent.
//!
//! ## What is NOT here, and why
//!
//! **Transport (play/pause/next).** Neither receiver exposes it in this image.
//! librespot v0.4.2 (see packages/librespot/librespot.mk) has no control API; it
//! takes `--onevent`/`--emit-sink-events` hooks only. shairport-sync is built
//! with the bare `BR2_PACKAGE_SHAIRPORT_SYNC=y` (board/ea/common/features/audio),
//! which does not select the D-Bus/MPRIS or metadata sub-options, so there is no
//! MPRIS object to call. Faking transport that is not there would be worse than
//! omitting it, so it is omitted. See the TODO at the bottom of this file for the
//! board-side wiring that would light it up.
//!
//! **Now-playing** is read from a small state file a receiver's `--onevent` hook
//! *could* write ([`NOWPLAYING_DIR`]). Absent file → `null`, nothing invented.
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Where librespot and shairport-sync land in this image. Checked for existence
/// to decide "installed"; the paths match the init scripts under the board
/// rootfs-overlay (S95librespot points `DAEMON` at the first of these).
const LIBRESPOT_BIN: &str = "/usr/bin/librespot";
const SHAIRPORT_BIN: &str = "/usr/bin/shairport-sync";

/// The file iod owns to tell the receivers which ALSA device to render to.
///
/// iod writes it; the receivers' init scripts must SOURCE it (that wiring is a
/// documented TODO for whoever owns board/ — see the end of this file). It is
/// shell so `. /etc/ohc/audio-output` just works, and the location is overridable
/// for tests and for boards whose /etc is read-only.
fn state_file() -> PathBuf {
    std::env::var_os("OHC_AUDIO_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/ohc/audio-output"))
}

/// Directory a receiver's `--onevent` hook may drop now-playing JSON into, one
/// file per receiver id (`librespot.json`, `shairport.json`). Volatile on
/// purpose — now-playing has no meaning across a reboot.
fn nowplaying_dir() -> PathBuf {
    std::env::var_os("OHC_AUDIO_NOWPLAYING_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/ohc"))
}

/// `/proc/asound/cards`, overridable so the parser can be tested off-box.
fn cards_path() -> PathBuf {
    std::env::var_os("OHC_ASOUND_CARDS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc/asound/cards"))
}

/// An ALSA playback device the UI can pick.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Output {
    /// What you pass to `aplay -D`, librespot `--device` or shairport `-- -d`:
    /// `hw:<cardid>`. The card *id* (the bracketed token), not the index, because
    /// the index moves with probe order and the id does not.
    pub id: String,
    /// Human name — the card's long description from `/proc/asound/cards`.
    pub name: String,
    /// The numeric card index, for `amixer -c <n>`.
    pub card: u32,
}

/// A network receiver (Spotify Connect, AirPlay).
#[derive(Serialize, Clone, Debug)]
pub struct Receiver {
    /// Topic slug: `librespot` / `shairport`.
    pub id: &'static str,
    /// What a person calls it.
    pub name: &'static str,
    /// The protocol, for an icon/label choice in the UI.
    pub kind: &'static str,
    /// Binary present on the rootfs.
    pub installed: bool,
    /// A process is actually up right now.
    pub running: bool,
    /// Honest capability flags: both false in this image. Here so the UI can
    /// light up controls the day a build enables MPRIS, without a protocol change.
    pub supports_transport: bool,
    pub supports_metadata: bool,
}

/// Parse `/proc/asound/cards`.
///
/// The format is two lines per card:
/// ```text
///  0 [DSP            ]: ADAU1451 - ADAU1451 analog
///                       ADAU1451 analog out on CE5300 I2S
/// ```
/// The first line gives the index and the bracketed *id*; the second is the long
/// name. Kept pure (takes the text) so it is unit-tested without a sound card.
pub fn parse_cards(text: &str) -> Vec<Output> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        // A card header line starts with optional spaces then a number then '['.
        let trimmed = line.trim_start();
        let Some((idx_str, rest)) = trimmed.split_once('[') else { continue };
        let Ok(card) = idx_str.trim().parse::<u32>() else { continue };
        let Some((id_raw, _)) = rest.split_once(']') else { continue };
        let id = id_raw.trim();
        if id.is_empty() {
            continue;
        }
        // Long name is the next (indented) line if there is one; fall back to the
        // short name after the ": " on the header line.
        let long = lines
            .peek()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string());
        if long.is_some() {
            lines.next();
        }
        let short = rest.split_once("]:").map(|(_, s)| s.trim().to_string());
        let name = long
            .or(short)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.to_string());
        out.push(Output { id: format!("hw:{id}"), name, card });
    }
    out
}

/// Every playback card the kernel has registered.
pub fn outputs() -> Vec<Output> {
    std::fs::read_to_string(cards_path())
        .map(|s| parse_cards(&s))
        .unwrap_or_default()
}

/// Is any process with this exact `comm` running?
///
/// Scans `/proc/<pid>/comm`, which is how a service-check with no daemon manager
/// to ask has to be done. `comm` is truncated to 15 bytes by the kernel, so the
/// names checked (`librespot`, `shairport-sync` → `shairport-sync`) are compared
/// against that truncation.
fn running(comm: &str) -> bool {
    let want = &comm.as_bytes()[..comm.len().min(15)];
    let Ok(dir) = std::fs::read_dir("/proc") else { return false };
    for e in dir.flatten() {
        if e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()).is_none() {
            continue;
        }
        if let Ok(c) = std::fs::read_to_string(e.path().join("comm")) {
            if c.trim().as_bytes() == want {
                return true;
            }
        }
    }
    false
}

/// Both receivers, with installed/running filled in. A receiver whose binary is
/// absent is still listed (installed:false) so the panel can say "not installed
/// on this image" rather than silently dropping it — but see [`capability`],
/// which omits the whole section when neither is installed.
pub fn receivers() -> Vec<Receiver> {
    vec![
        Receiver {
            id: "librespot",
            name: "Spotify Connect",
            kind: "spotify",
            installed: Path::new(LIBRESPOT_BIN).exists(),
            running: running("librespot"),
            // librespot v0.4.2: hooks only, no control/metadata surface we read.
            supports_transport: false,
            supports_metadata: false,
        },
        Receiver {
            id: "shairport",
            name: "AirPlay",
            kind: "airplay",
            // comm is truncated to 15 bytes: "shairport-sync" is 14, so it fits.
            installed: Path::new(SHAIRPORT_BIN).exists(),
            running: running("shairport-sync"),
            // Built without --with-dbus/--with-mpris/--with-metadata in this image.
            supports_transport: false,
            supports_metadata: false,
        },
    ]
}

/// The selected output device string, read back from the state file. `None`
/// means nothing has been chosen and the receivers follow the default PCM (which
/// is exactly what S95librespot does today: `--backend alsa` with no device).
pub fn selected() -> Option<String> {
    let text = std::fs::read_to_string(state_file()).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        if k.trim() == "OHC_AUDIO_DEVICE" {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Write the selected device to the state file, after checking it is one the
/// board actually has. Creates the parent directory so a fresh box works.
///
/// Returns the device written. The receivers do not pick this up until they are
/// restarted AND their init scripts source the file — see the TODO; iod has no
/// business restarting another package's daemon, so it does not.
pub fn select(device: &str) -> Result<String, String> {
    let known = outputs();
    if !known.iter().any(|o| o.id == device) {
        return Err(format!(
            "no such output {device:?}; have: {}",
            known.iter().map(|o| o.id.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    let path = state_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = format!(
        "# Written by iod (audio output selection). Sourced by the receiver init\n\
         # scripts. Do not edit by hand — the Audio panel owns this file.\n\
         OHC_AUDIO_DEVICE=\"{device}\"\n"
    );
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(device.to_string())
}

/// Resolve a device string (or the selection, or the first output) to a card
/// index for `amixer -c`.
fn card_of(device: Option<&str>) -> Option<u32> {
    let outs = outputs();
    let want = device.map(str::to_string).or_else(selected);
    if let Some(w) = want {
        if let Some(o) = outs.iter().find(|o| o.id == w) {
            return Some(o.card);
        }
        // Accept a bare `hw:N` / card index too.
        if let Some(n) = w.strip_prefix("hw:").and_then(|s| s.parse::<u32>().ok()) {
            return Some(n);
        }
    }
    outs.first().map(|o| o.card)
}

/// Mixer controls to try, in order. ALSA does not name a "volume" control
/// portably — the ADAU1451 may expose `Master`, a USB DAC `PCM`, an HDA codec
/// `Master` — so the first one that answers wins. UNVERIFIED against the EA3's
/// ADAU1451, which is probe-only in this image (see board.env).
const VOLUME_CONTROLS: &[&str] = &["Master", "PCM", "Digital", "Speaker", "Playback"];

/// Read the current volume percent on a card, best-effort. Parses the `[NN%]`
/// amixer prints. `None` if amixer is missing, no control answered, or the output
/// could not be parsed — the UI then simply does not show a volume.
pub async fn volume_get(device: Option<&str>) -> Option<u8> {
    let card = card_of(device)?;
    for ctl in VOLUME_CONTROLS {
        let out = tokio::process::Command::new("amixer")
            .args(["-c", &card.to_string(), "sget", ctl])
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(pct) = parse_amixer_pct(&text) {
            return Some(pct);
        }
    }
    None
}

/// Set the volume percent on a card, best-effort across the same control list.
/// Returns the control that took it, or an error if none did.
pub async fn volume_set(device: Option<&str>, percent: u8) -> Result<(u32, &'static str), String> {
    let card = card_of(device).ok_or_else(|| "no ALSA output to set volume on".to_string())?;
    let pct = percent.min(100);
    let mut last = String::from("amixer not found or no usable control");
    for ctl in VOLUME_CONTROLS {
        match tokio::process::Command::new("amixer")
            .args(["-c", &card.to_string(), "sset", ctl, &format!("{pct}%")])
            .output()
            .await
        {
            Ok(o) if o.status.success() => return Ok((card, ctl)),
            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => return Err(format!("amixer: {e}")),
        }
    }
    Err(last)
}

/// Pull the first `[NN%]` out of amixer's output.
pub fn parse_amixer_pct(text: &str) -> Option<u8> {
    let i = text.find('[')?;
    let rest = &text[i + 1..];
    let j = rest.find('%')?;
    rest[..j].parse::<u8>().ok()
}

/// Now-playing for a receiver, if its hook left us a file. The file is whatever
/// JSON the hook writes; iod passes it through rather than imposing a schema it
/// cannot fill from the receivers it has. `None` = nothing playing / no hook.
pub fn now_playing(receiver_id: &str) -> Option<Value> {
    let path = nowplaying_dir().join(format!("{receiver_id}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The audio capability section, or `None` when the box has no audio at all.
///
/// Present when there is at least one output OR at least one receiver installed —
/// a box with a sound card but no receivers can still have its volume set, and a
/// box with receivers but a not-yet-probed card can still show their status.
pub fn capability() -> Option<Value> {
    let outputs = outputs();
    let receivers = receivers();
    let any_receiver = receivers.iter().any(|r| r.installed);
    if outputs.is_empty() && !any_receiver {
        return None;
    }
    Some(json!({
        "outputs": outputs,
        "receivers": receivers,
        "selected": selected(),
    }))
}

/// The full live status, for the REST read and the MQTT state mirror. Builds on
/// [`capability`] and adds the things that change at runtime: volume, and each
/// receiver's now-playing. Returns `None` on a box with no audio.
pub async fn status() -> Option<Value> {
    let mut v = capability()?;
    let obj = v.as_object_mut().unwrap();
    if let Some(vol) = volume_get(None).await {
        obj.insert("volume".into(), json!(vol));
    }
    // Attach now-playing to each receiver that has some.
    if let Some(recv) = obj.get_mut("receivers").and_then(|r| r.as_array_mut()) {
        for r in recv.iter_mut() {
            if let Some(id) = r.get("id").and_then(|i| i.as_str()).map(str::to_string) {
                if let Some(np) = now_playing(&id) {
                    if let Some(ro) = r.as_object_mut() {
                        ro.insert("now_playing".into(), np);
                    }
                }
            }
        }
    }
    Some(v)
}

// ─────────────────────────────────────────────────────────────────────────────
// TODO (board-side, for whoever owns board/ — iod must not edit init scripts):
//
//   1. The receiver init scripts must source iod's selection file so a pick in
//      the UI actually moves the audio. DONE for librespot (packages/librespot/
//      S95librespot now sources /etc/ohc/audio-output and adds --device). Still
//      TODO for shairport-sync, whose init is Buildroot's: pass -d "$OHC_AUDIO_DEVICE"
//      (e.g. via an init override or by templating /etc/shairport-sync.conf's
//      alsa output_device from the same file). Until then its output follows the
//      default PCM and the panel's "restart to apply" note stands for it.
//      NOTE: the audio userspace is board/ea/common/features/audio (all EA); the
//      EA3 DSP kernel half is board/ea/common/features/audio-dsp.
//
//   2. now-playing + transport: build shairport-sync with --with-mpris-interface
//      (BR2 sub-option) OR add a librespot `--onevent` hook that writes
//      /run/ohc/<id>.json. iod already reads that file (now_playing) and already
//      carries supports_transport/supports_metadata flags; flipping them true and
//      adding the MPRIS calls is then an iod change with a real surface behind it.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cards_reads_index_id_and_longname() {
        // The real shape of /proc/asound/cards: header line then an indented
        // long-name line.
        let text = "\
 0 [DSP            ]: ADAU1451 - ADAU1451 analog
                      ADAU1451 analog out on CE5300 I2S
 1 [HDMI           ]: HDA-Intel - HDA ATI HDMI
                      HDA ATI HDMI at 0xf0040000
";
        let o = parse_cards(text);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0], Output { id: "hw:DSP".into(), name: "ADAU1451 analog out on CE5300 I2S".into(), card: 0 });
        assert_eq!(o[1].id, "hw:HDMI");
        assert_eq!(o[1].card, 1);
    }

    #[test]
    fn parse_cards_falls_back_to_short_name_without_a_longname_line() {
        let text = " 0 [DSP            ]: ADAU1451 - short only\n";
        let o = parse_cards(text);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].name, "ADAU1451 - short only");
    }

    #[test]
    fn parse_cards_empty_when_no_sound() {
        // `/proc/asound/cards` on a box with no card is "--- no soundcards ---".
        assert!(parse_cards("--- no soundcards ---\n").is_empty());
        assert!(parse_cards("").is_empty());
    }

    #[test]
    fn amixer_percent_is_pulled_from_brackets() {
        let sample = "  Front Left: Playback 180 [71%] [-18.00dB] [on]";
        assert_eq!(parse_amixer_pct(sample), Some(71));
        assert_eq!(parse_amixer_pct("no brackets here"), None);
    }
}
