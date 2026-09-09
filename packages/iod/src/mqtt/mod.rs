//! MQTT — the IO surface.
//!
//! IO control speaks MQTT and nothing else. The config GUI, Home Assistant, a
//! Node-RED flow and a shell script all use the same topics, so there is one
//! set of semantics to document, one to test, and one to get right. REST keeps
//! the things MQTT is bad at: what the board IS, and its configuration.
//!
//! Two independent roles, both optional, neither implying the other:
//!
//! * **serve** — iod carries its own topics, over a WebSocket for the browser
//!   and plain TCP for everything else. This is what the config GUI talks to.
//! * **bridge** — iod also connects OUT to somebody else's broker.
//!
//! They are independent on purpose. If the GUI depended on the house broker,
//! then a broker that was down or misconfigured would take out the very screen
//! you would use to fix it.
pub mod client;
pub mod server;
pub mod settings;
pub mod topics;

use crate::ops::Cmd;
use crate::Config;
use std::sync::Arc;
use tokio::sync::watch;

/// Start both roles, and restart them whenever the settings change.
///
/// A settings save has to take effect without an init-script restart: the
/// operator changing the broker is very often doing it THROUGH the GUI, and
/// telling them to SSH in and bounce the daemon to apply their own edit would
/// be a poor way to ship a settings page.
///
/// Restarting is done by dropping the tasks and starting new ones rather than
/// mutating a live client, because a half-applied broker change — new host,
/// old credentials — fails in ways that are tedious to diagnose.
pub async fn supervise(cfg: Arc<Config>, mut rx: watch::Receiver<settings::Mqtt>) {
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    loop {
        for t in tasks.drain(..) {
            t.abort();
        }
        let m = rx.borrow().clone();

        if m.serve {
            if m.listen_port > 0 {
                tasks.push(tokio::spawn(server::listen_tcp(cfg.clone(), m.clone(), m.listen_port)));
            }
            // The WebSocket endpoint needs no task: webd proxies it and axum
            // spawns a session per connection.
            eprintln!("iod/mqttd: serving topics under {}/{}", m.prefix, m.client_id);
        } else {
            // Worth saying out loud. With this off the config GUI's IO panels
            // have nothing to talk to, and the symptom is a page that loads
            // fine and then does nothing.
            eprintln!("iod/mqttd: NOT serving — the config GUI's IO panels will not work");
        }

        if m.bridge && !m.url.is_empty() {
            eprintln!("iod/mqtt: bridging to {}", m.url);
            tasks.push(tokio::spawn(client::run(cfg.clone(), m.clone())));
        }

        if rx.changed().await.is_err() {
            return;
        }
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
        ["mcu", "reset"] => Some(Cmd::McuReset),
        ["ir", n, "send"] => {
            Some(Cmd::IrSend { port: topics::index(n)?, pronto: b.to_string(), repeat: 1 })
        }
        ["serial", n, "write"] => Some(Cmd::SerialWrite {
            index: topics::index(n)? as usize,
            data: body.to_string(),
            hex: false,
            b64: false,
        }),
        ["serial", n, "baud"] => Some(Cmd::SerialBaud {
            index: topics::index(n)? as usize,
            baud: b.parse().ok()?,
        }),
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
        assert!(parse_cmd("nonsense/thing", "1").is_none());
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
    fn raw_takes_any_command() {
        let c = parse_cmd("raw", r#"{"op":"serial.baud","index":0,"baud":9600}"#);
        assert!(matches!(c, Some(Cmd::SerialBaud { index: 0, baud: 9600 })));
    }
}
