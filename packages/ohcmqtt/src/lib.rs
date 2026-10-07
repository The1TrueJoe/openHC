//! Shared MQTT plumbing for openHC daemons.
//!
//! Every board runs mosquitto; every daemon (iod, sysmond, ohc-audiod) is a
//! client of it and publishes its OWN topics under one tree, `<prefix>/<host>/`.
//! This crate is the part they must agree on, so it lives in one place:
//!
//! * the topic base — iod's MQTT settings decide it (`prefix`, `client_id`,
//!   defaulting to `openhc` / the hostname), and every daemon reads the same
//!   file so a renamed tree moves all of them together;
//! * the connection — each daemon is its own client (`<client_id>-<service>`)
//!   with a retained last will on `<base>/status/<service>`, so an integration
//!   can tell which part of the box went away;
//! * the payload rules — scalars bare (`ON`/`OFF`, numbers, plain strings, for
//!   shell scripts and Home Assistant), anything structured as JSON;
//! * retained state — published only when it changes, and all of it again on
//!   every (re)connect, because the broker keeps retained state in RAM.
use rumqttc::{AsyncClient, LastWill, MqttOptions, QoS};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// `<prefix>/<client_id>` — the root every topic hangs off.
#[derive(Clone, Debug)]
pub struct Base {
    pub prefix: String,
    pub client_id: String,
}

#[derive(serde::Deserialize, Default)]
struct IodSettings {
    #[serde(default)]
    mqtt: IodMqtt,
}
#[derive(serde::Deserialize, Default)]
struct IodMqtt {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    client_id: String,
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "openhc".into())
}

impl Base {
    /// The same answer iod gives: its settings file (`IOD_SETTINGS`, else
    /// `/data/ohc/iod.json` or `/etc/openhc/iod.json`, whichever exists), then
    /// the environment (`IOD_MQTT_PREFIX`, `IOD_MQTT_CLIENT_ID`), then
    /// `openhc` / the hostname.
    pub fn resolve() -> Base {
        let path = std::env::var("IOD_SETTINGS").ok().unwrap_or_else(|| {
            ["/data/ohc/iod.json", "/etc/openhc/iod.json"]
                .into_iter()
                .find(|p| std::path::Path::new(p).exists())
                .unwrap_or("/etc/openhc/iod.json")
                .to_string()
        });
        let s: IodSettings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let pick = |env: &str, file: String, dflt: String| {
            std::env::var(env).ok().filter(|v| !v.is_empty())
                .or(Some(file).filter(|v| !v.is_empty()))
                .unwrap_or(dflt)
        };
        Base {
            prefix: pick("IOD_MQTT_PREFIX", s.mqtt.prefix, "openhc".into()),
            client_id: pick("IOD_MQTT_CLIENT_ID", s.mqtt.client_id, hostname()),
        }
    }

    pub fn root(&self) -> String {
        format!("{}/{}", self.prefix, self.client_id)
    }
    /// `<base>/state/<path>` — retained state.
    pub fn state(&self, path: &str) -> String {
        format!("{}/state/{path}", self.root())
    }
    /// `<base>/cmd/<path>` — commands (subscribe with `cmd("audio/#")`).
    pub fn cmd(&self, path: &str) -> String {
        format!("{}/cmd/{path}", self.root())
    }
}

/// The box's broker: `OHC_MQTT_BROKER` (`host:port`), default loopback 1883.
pub fn broker() -> (String, u16) {
    let s = std::env::var("OHC_MQTT_BROKER").unwrap_or_else(|_| "127.0.0.1:1883".into());
    match s.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(1883)),
        None => (s, 1883),
    }
}

/// Connection options for `service`: its own client id, a 30 s keepalive, and a
/// retained `offline` last will on `<base>/status/<service>`.
pub fn options(service: &str, base: &Base) -> MqttOptions {
    let (host, port) = broker();
    let mut o = MqttOptions::new(format!("{}-{service}", base.client_id), host, port);
    o.set_keep_alive(Duration::from_secs(30));
    o.set_last_will(LastWill::new(status_topic(service, base), "offline", QoS::AtLeastOnce, true));
    o
}

pub fn status_topic(service: &str, base: &Base) -> String {
    format!("{}/status/{service}", base.root())
}

/// Scalars bare, everything structured as JSON — the rule iod has always used,
/// and the one the web UI's decoder undoes.
pub fn payload(v: &Value) -> String {
    match v {
        Value::Bool(b) => (if *b { "ON" } else { "OFF" }).into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Retained state for one daemon: publishes a path only when its payload
/// changed, remembers everything, and republishes it all after a reconnect.
#[derive(Default)]
pub struct Retained {
    last: HashMap<String, String>,
}

impl Retained {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `<base>/state/<path>`. Returns whether anything was sent.
    pub async fn set(&mut self, client: &AsyncClient, base: &Base, path: &str, v: &Value) -> bool {
        let body = payload(v);
        if self.last.get(path) == Some(&body) {
            return false;
        }
        let ok = client.publish(base.state(path), QoS::AtLeastOnce, true, body.clone()).await.is_ok();
        if ok {
            self.last.insert(path.to_string(), body);
        }
        ok
    }

    /// Everything again — call on every ConnAck (the broker keeps retained
    /// state in RAM, and a restarted broker has none).
    pub async fn republish(&self, client: &AsyncClient, base: &Base) {
        for (path, body) in &self.last {
            let _ = client.publish(base.state(path), QoS::AtLeastOnce, true, body.clone()).await;
        }
    }
}

/// Announce `<base>/status/<service>` = online (retained); pair with the last
/// will from [`options`].
pub async fn online(client: &AsyncClient, service: &str, base: &Base) {
    let _ = client.publish(status_topic(service, base), QoS::AtLeastOnce, true, "online").await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_rules() {
        assert_eq!(payload(&serde_json::json!(true)), "ON");
        assert_eq!(payload(&serde_json::json!(42)), "42");
        assert_eq!(payload(&serde_json::json!("x y")), "x y");
        assert_eq!(payload(&serde_json::json!([1, 2])), "[1,2]");
    }
    #[test]
    fn topics() {
        let b = Base { prefix: "openhc".into(), client_id: "box".into() };
        assert_eq!(b.state("audio/map"), "openhc/box/state/audio/map");
        assert_eq!(b.cmd("audio/#"), "openhc/box/cmd/audio/#");
        assert_eq!(status_topic("audiod", &b), "openhc/box/status/audiod");
    }
}
