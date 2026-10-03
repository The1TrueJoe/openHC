//! MQTT — the IO surface.
//!
//! IO control speaks MQTT and nothing else. The config GUI, Home Assistant, a
//! Node-RED flow and a shell script all use the same topics, so there is one
//! set of semantics to document, one to test, and one to get right. REST keeps
//! the things MQTT is bad at: what the board IS, and its configuration.
//!
//! Every board runs mosquitto. iod is one client of it (client.rs), beside
//! ohc-audiod and sysmond, each publishing its own topics under the same tree;
//! the web UI reaches the broker over a WebSocket that webd proxies at /mqtt.
//! Reaching a house broker is mosquitto's bridge, configured from iod's
//! settings (bridge.rs).
pub mod bridge;
pub mod client;
pub mod settings;
pub mod topics;

use crate::ops::Cmd;
use crate::Config;
use std::sync::Arc;
use tokio::sync::watch;

/// Keep the bridge and the client in line with the settings, restarting the
/// client whenever they change (a renamed prefix/client id moves the tree).
///
/// A settings save has to take effect without an init-script restart: the
/// operator changing the broker is very often doing it THROUGH the GUI.
pub async fn supervise(cfg: Arc<Config>, mut rx: watch::Receiver<settings::Mqtt>) {
    loop {
        let m = rx.borrow().clone();
        bridge::apply(&m).await;
        let task = tokio::task::spawn_local(client::run(cfg.clone(), m));
        if rx.changed().await.is_err() {
            return;
        }
        task.abort();
        eprintln!("iod/mqtt: settings changed, restarting");
    }
}


/// Map a command topic tail plus its payload to a [`Cmd`].
///
/// Payloads are the plain strings an automation tool sends — `ON`, `OFF`,
/// `TOGGLE` — because half of what publishes here will be a Home Assistant
/// switch or a one-line shell script, not a JSON encoder.
pub fn parse_cmd(tail: &str, body: &str) -> Option<Cmd> {
    let parts: Vec<&str> = tail.split('/').collect();
    let b = body.trim();
    match parts.as_slice() {
        // The escape hatch: any command, verbatim, as JSON.
        ["raw"] => serde_json::from_str(b).ok(),
        ["relay", n, "set"] => {
            let index = topics::index(n)?;
            match b.to_ascii_uppercase().as_str() {
                "ON" | "TRUE" | "1" => Some(Cmd::RelaySet { index, on: true }),
                "OFF" | "FALSE" | "0" => Some(Cmd::RelaySet { index, on: false }),
                "TOGGLE" => Some(Cmd::RelayToggle { index }),
                _ => None,
            }
        }
        // led/<slug>/set — ON/OFF, or a number for brightness on a LED that has
        // more than two levels. The slug is the kernel's function name, not an
        // index, because LEDs are not a numbered row on the panel.
        ["led", name, "set"] => match b.to_ascii_uppercase().as_str() {
            "ON" | "TRUE" => Some(Cmd::LedSet { name: name.to_string(), on: Some(true), level: None }),
            "OFF" | "FALSE" => Some(Cmd::LedSet { name: name.to_string(), on: Some(false), level: None }),
            _ => b.parse::<u32>().ok().map(|l| Cmd::LedSet {
                name: name.to_string(),
                on: None,
                level: Some(l),
            }),
        },
        ["mcu", "reset"] => Some(Cmd::McuReset),
        ["restore", "status"] => Some(Cmd::RestoreStatus),
        // Body must be CONFIRM — a bare topic publish cannot wipe the box.
        ["restore", "stock"] => Some(Cmd::RestoreStock {
            confirm: b.trim().eq_ignore_ascii_case("confirm"),
        }),
        // `ir/front/send` for the internal blaster; `ir/<n>/send` for a rear
        // jack, numbered as the case is.
        ["ir", "front", "send"] => {
            Some(Cmd::IrSend { port: crate::ops::IrTarget::Front, pronto: b.to_string(), repeat: 1 })
        }
        ["ir", n, "send"] => Some(Cmd::IrSend {
            port: crate::ops::IrTarget::Jack(n.parse().ok().filter(|&x| x > 0)?),
            pronto: b.to_string(),
            repeat: 1,
        }),
        ["serial", n, "write"] => Some(Cmd::SerialWrite {
            index: topics::index(n)? as usize,
            data: body.to_string(),
            hex: false,
            b64: false,
        }),
        ["serial", n, l @ ("dtr" | "rts")] => Some(Cmd::SerialLine {
            index: topics::index(n)? as usize,
            line: if *l == "dtr" { crate::ops::SerialLine::Dtr } else { crate::ops::SerialLine::Rts },
            on: match b.to_ascii_uppercase().as_str() {
                "ON" | "TRUE" | "1" => true,
                "OFF" | "FALSE" | "0" => false,
                _ => return None,
            },
        }),
        ["serial", n, "baud"] => Some(Cmd::SerialBaud {
            index: topics::index(n)? as usize,
            baud: b.parse().ok()?,
        }),
        // Return to stock is one-way, so the payload must literally say so.
        ["system", "restore"] if b == "confirm" => Some(Cmd::RestoreStock { confirm: true }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_payloads_are_understood() {
        // Topics carry the number on the PANEL, so relay/1 is the first relay
        // and reaches internal index 0.
        assert!(matches!(parse_cmd("relay/1/set", "ON"), Some(Cmd::RelaySet { index: 0, on: true })));
        assert!(matches!(parse_cmd("relay/4/set", "off"), Some(Cmd::RelaySet { index: 3, on: false })));
        assert!(matches!(parse_cmd("relay/2/set", " TOGGLE\n"), Some(Cmd::RelayToggle { index: 1 })));
        // A Home Assistant switch sends exactly these, so a typo here is a
        // relay that silently never moves.
        assert!(parse_cmd("relay/1/set", "maybe").is_none());
        // LEDs are addressed by name, and take a level as well as on/off.
        assert!(matches!(parse_cmd("led/power/set", "ON"),
                         Some(Cmd::LedSet { on: Some(true), .. })));
        assert!(matches!(parse_cmd("led/wifi-blue/set", "0"),
                         Some(Cmd::LedSet { level: Some(0), .. })));
        assert!(parse_cmd("led/power/set", "sideways").is_none());
        assert!(parse_cmd("nonsense/thing", "1").is_none());
    }

    #[test]
    fn the_front_emitter_is_named_not_numbered() {
        use crate::ops::IrTarget;
        assert!(matches!(parse_cmd("ir/front/send", "0000 006d 0001 0000 0016 0016"),
                         Some(Cmd::IrSend { port: IrTarget::Front, .. })));
        assert!(matches!(parse_cmd("ir/1/send", "0000 006d 0001 0000 0016 0016"),
                         Some(Cmd::IrSend { port: IrTarget::Jack(1), .. })));
        assert!(matches!(parse_cmd("ir/6/send", "0000 006d 0001 0000 0016 0016"),
                         Some(Cmd::IrSend { port: IrTarget::Jack(6), .. })));
    }

    #[test]
    fn zero_is_not_a_label_any_hardware_carries() {
        // Far more likely to be a client that assumed zero-based than a real
        // request, and silently driving relay 1 for it would be the worst
        // possible answer.
        assert!(parse_cmd("relay/0/set", "ON").is_none());
        assert!(parse_cmd("ir/0/send", "0000 006d").is_none());
        assert!(parse_cmd("serial/0/baud", "9600").is_none());
    }

    #[test]
    fn serial_lines_parse() {
        use crate::ops::SerialLine;
        assert!(matches!(parse_cmd("serial/1/dtr", "OFF"),
                         Some(Cmd::SerialLine { index: 0, line: SerialLine::Dtr, on: false })));
        assert!(matches!(parse_cmd("serial/4/rts", "on"),
                         Some(Cmd::SerialLine { index: 3, line: SerialLine::Rts, on: true })));
        assert!(parse_cmd("serial/1/dtr", "maybe").is_none());
    }

    #[test]
    fn restore_needs_the_literal_confirm() {
        assert!(matches!(parse_cmd("system/restore", "confirm"), Some(Cmd::RestoreStock { confirm: true })));
        assert!(parse_cmd("system/restore", "yes").is_none());
        // Audio and health belong to their own daemons now.
        assert!(parse_cmd("audio/volume", "60").is_none());
        assert!(parse_cmd("health/fan", "auto").is_none());
    }

    #[test]
    fn raw_takes_any_command() {
        let c = parse_cmd("raw", r#"{"op":"serial.baud","index":0,"baud":9600}"#);
        assert!(matches!(c, Some(Cmd::SerialBaud { index: 0, baud: 9600 })));
    }
}
