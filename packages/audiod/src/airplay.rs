//! Moving an AirPlay sender's volume slider from here.
//!
//! Classic AirPlay only carries volume sender → receiver. The way back is DACP,
//! the remote-control channel a sender (iPhone, Music) opens to the receiver:
//! shairport-sync's D-Bus interface (packages/ohc-shairport-sync) exposes it
//! as `RemoteControl.SetAirplayVolume(-30 … 0)`, which tells the sender to set
//! its volume for this receiver. The sender then sends that volume back as
//! usual, and because openHC's level and shairport-sync's mixer share one curve
//! (levels::airplay_volume) the echo lands on the level that was just set.
//!
//! Every endpoint is its own shairport-sync on the system bus: the first to
//! start owns `org.gnome.ShairportSync`, each later one
//! `org.gnome.ShairportSync.i<pid>` (packages/ohc-shairport-sync patch 0001 —
//! 5.x dropped that fallback; the ohc-shairport.conf D-Bus policy lets them). The
//! bus says which pid owns the plain name. Best effort: with no sender
//! connected, or one without DACP, there is nothing to move.
//!
//! The other half is where a session STARTS: AirPlay applies, and reports to a
//! connecting sender as its `initialVolume`, the receiver's last volume —
//! −24 (20%) on a fresh start — which would yank the output to it the moment a
//! phone connects. `set_start_volume` keeps it at the output's level instead
//! (the `Volume` D-Bus property; patch 0002 makes setting it do that).
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const NAME: &str = "org.gnome.ShairportSync";

fn dbus_send(args: &[&str]) -> Option<String> {
    let o = Command::new("dbus-send").args(["--system", "--print-reply", "--reply-timeout=2000"]).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The bus name the shairport-sync with this pid answers on.
fn name_for(pid: u32) -> String {
    let owner = dbus_send(&[
        "--dest=org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.GetConnectionUnixProcessID",
        &format!("string:{NAME}"),
    ]);
    if owner.as_deref().and_then(parse_uint32) == Some(pid) {
        NAME.to_string()
    } else {
        format!("{NAME}.i{pid}")
    }
}

/// `   uint32 1234` (dbus-send --print-reply) → 1234.
fn parse_uint32(reply: &str) -> Option<u32> {
    reply.split_whitespace().skip_while(|w| *w != "uint32").nth(1)?.parse().ok()
}

/// Ask the sender playing to this shairport-sync to set its volume to
/// `percent` (openHC's level; on the shared curve that is also the sender's
/// slider position). Two channels, because the sessions differ:
///   * AirPlay 2: `AdvancedRemoteControl.SetVolume(percent)` — a `dvlc`
///     command over the session's event channel. AirPlay 2 senders open no
///     DACP server, so the classic call reaches nothing for them.
///   * classic AirPlay: `RemoteControl.SetAirplayVolume(-30 … 0)` over DACP.
///
/// Whichever has no session does nothing. Runs off the caller's thread.
pub fn set_sender_volume(pid: u32, percent: u8) {
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        let dest = format!("--dest={}", name_for(pid));
        let _ = dbus_send(&[
            &dest,
            "/org/gnome/ShairportSync",
            "org.gnome.ShairportSync.AdvancedRemoteControl.SetVolume",
            &format!("int32:{}", percent.min(100)),
        ]);
        let _ = dbus_send(&[
            &dest,
            "/org/gnome/ShairportSync",
            "org.gnome.ShairportSync.RemoteControl.SetAirplayVolume",
            &format!("double:{:.6}", crate::levels::airplay_volume(percent)),
        ]);
    });
}

/// Make `volume` the receiver's starting volume (see the module comment). Off
/// the caller's thread, and retried for a few seconds: a receiver that has
/// just started may not have its name on the bus yet.
pub fn set_start_volume(pid: u32, volume: f64) {
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..10 {
            let set = dbus_send(&[
                &format!("--dest={}", name_for(pid)),
                "/org/gnome/ShairportSync",
                "org.freedesktop.DBus.Properties.Set",
                "string:org.gnome.ShairportSync",
                "string:Volume",
                &format!("variant:double:{volume:.6}"),
            ]);
            if set.is_some() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

/// `set_start_volume` for a receiver being started now: waits for its process.
pub fn set_start_volume_when_up(pid: Arc<AtomicU32>, volume: f64) {
    std::thread::spawn(move || {
        for _ in 0..20 {
            let p = pid.load(Ordering::Relaxed);
            if p != 0 {
                set_start_volume(p, volume);
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_the_owner_pid() {
        let r = "method return time=1.0 sender=org.freedesktop.DBus -> destination=:1.9 serial=3 reply_serial=2\n   uint32 1234\n";
        assert_eq!(parse_uint32(r), Some(1234));
        assert_eq!(parse_uint32("Error org.freedesktop.DBus.Error.NameHasNoOwner"), None);
    }
}
