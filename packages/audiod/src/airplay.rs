//! Moving an AirPlay sender's volume slider from here.
//!
//! Classic AirPlay only carries volume sender → receiver. The way back is DACP,
//! the remote-control channel a sender (iPhone, Music) opens to the receiver:
//! shairport-sync's D-Bus interface (BR2_PACKAGE_SHAIRPORT_SYNC_DBUS) exposes it
//! as `RemoteControl.SetAirplayVolume(-30 … 0)`, which tells the sender to set
//! its volume for this receiver. The sender then sends that volume back as
//! usual, and because openHC's level and shairport-sync's mixer share one curve
//! (levels::airplay_volume) the echo lands on the level that was just set.
//!
//! Every endpoint is its own shairport-sync on the system bus: the first to
//! start owns `org.gnome.ShairportSync`, each later one
//! `org.gnome.ShairportSync.i<pid>` (dbus-ohc-shairport.conf lets them). The
//! bus says which pid owns the plain name. Best effort: with no sender
//! connected, or one without DACP, there is nothing to move.
use std::process::Command;

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

/// Ask the sender playing to this shairport-sync to set its volume. Runs off
/// the caller's thread: the D-Bus round trips and DACP can take a moment.
pub fn set_sender_volume(pid: u32, volume: f64) {
    if pid == 0 {
        return;
    }
    std::thread::spawn(move || {
        let dest = format!("--dest={}", name_for(pid));
        // Fails harmlessly when no sender is connected (or it has no DACP).
        let _ = dbus_send(&[
            &dest,
            "/org/gnome/ShairportSync",
            "org.gnome.ShairportSync.RemoteControl.SetAirplayVolume",
            &format!("double:{volume:.6}"),
        ]);
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
