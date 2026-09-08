//! The MQTT bridge — iod as seen by an external control system.
//!
//! A bridge, deliberately, not the core. A browser cannot speak raw MQTT, a
//! controller must work with no broker on the network at all, and
//! request/response with correlation is awkward over pub/sub. So the control
//! socket stays native and this republishes the same bus outward for the world
//! that is already MQTT-shaped: Home Assistant, Node-RED, openHAB, a PLC.
//!
//! The mapping is almost free because the bus already draws the distinction
//! MQTT cares about:
//!
//! * **state** → `<prefix>/<id>/state/<path>`, RETAINED. A subscriber that
//!   connects an hour late is immediately told every relay and contact.
//! * **events** → `<prefix>/<id>/event/<topic>`, NOT retained. Replaying "a
//!   remote was pressed" to whoever connects next would be a lie.
//!
//! Getting that backwards is the classic MQTT integration bug: retained events
//! re-fire automations on every reconnect, and unretained state leaves a
//! restarted consumer blind until something happens to change.
use crate::events::Msg;
use crate::ops::{self, Cmd};
use crate::Config;
use rumqttc::{AsyncClient, Event as MqEvent, Incoming, LastWill, MqttOptions, QoS, TlsConfiguration, Transport};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

pub struct Settings {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub user: Option<String>,
    pub pass: Option<String>,
    /// PEM bundle for a private CA. Without it, TLS verifies against whatever
    /// roots the image ships — which on a controller may be nothing, so a
    /// self-signed broker MUST set this rather than being silently trusted.
    pub ca: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
    pub prefix: String,
    pub client_id: String,
    /// Home Assistant discovery prefix. Empty disables it.
    pub discovery: String,
}

impl Settings {
    /// Read from the environment, which `board.env` feeds.
    /// Returns `None` when no broker is configured — the overwhelmingly common
    /// case, and not an error.
    pub fn from_env(hostname: &str) -> Option<Settings> {
        let url = std::env::var("IOD_MQTT_URL").ok().filter(|s| !s.trim().is_empty())?;
        let (scheme, rest) = url.split_once("://").unwrap_or(("mqtt", url.as_str()));
        let tls = scheme.eq_ignore_ascii_case("mqtts") || scheme.eq_ignore_ascii_case("ssl");
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(if tls { 8883 } else { 1883 })),
            None => (rest.to_string(), if tls { 8883 } else { 1883 }),
        };
        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Some(Settings {
            host,
            port,
            tls,
            user: env("IOD_MQTT_USER"),
            pass: env("IOD_MQTT_PASS"),
            ca: env("IOD_MQTT_CA"),
            client_cert: env("IOD_MQTT_CLIENT_CERT"),
            client_key: env("IOD_MQTT_CLIENT_KEY"),
            prefix: env("IOD_MQTT_PREFIX").unwrap_or_else(|| "openhc".into()),
            client_id: env("IOD_MQTT_CLIENT_ID").unwrap_or_else(|| hostname.to_string()),
            discovery: std::env::var("IOD_MQTT_DISCOVERY").unwrap_or_else(|_| "homeassistant".into()),
        })
    }
}

fn read_pem(path: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// The system CA bundle, for a broker with a publicly-issued certificate.
///
/// A minimal Buildroot image often has none of these, which is exactly why the
/// caller turns a miss into a clear message rather than a TLS error.
fn system_roots() -> Option<Vec<u8>> {
    const PATHS: [&str; 4] = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/usr/share/ca-certificates/ca-certificates.crt",
    ];
    PATHS.iter().find_map(|p| std::fs::read(p).ok()).filter(|b| !b.is_empty())
}

/// Run the bridge until the process ends, reconnecting on its own.
pub async fn run(cfg: Arc<Config>, s: Settings) {
    if s.tls {
        // rustls is built here with no default crypto provider (see Cargo.toml
        // — the default one needs a C toolchain this workspace does not have),
        // so the provider has to be chosen explicitly before any TLS handshake.
        // Err just means something already installed one.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    }
    let base = format!("{}/{}", s.prefix, s.client_id);
    let mut opts = MqttOptions::new(&s.client_id, &s.host, s.port);
    opts.set_keep_alive(Duration::from_secs(30));

    if let (Some(u), Some(p)) = (&s.user, &s.pass) {
        opts.set_credentials(u.clone(), p.clone());
    }

    if s.tls {
        // Client auth is optional; a broker that wants mutual TLS gets it, one
        // that only wants a username still works.
        let client_auth = match (&s.client_cert, &s.client_key) {
            (Some(c), Some(k)) => match (read_pem(c), read_pem(k)) {
                (Ok(c), Ok(k)) => Some((c, k)),
                _ => {
                    eprintln!("iod/mqtt: cannot read client cert/key; continuing without client auth");
                    None
                }
            },
            _ => None,
        };
        let ca = match &s.ca {
            Some(p) => match read_pem(p) {
                Ok(b) => b,
                Err(e) => {
                    // Refuse rather than silently falling back to no
                    // verification: an operator who configured a CA asked for
                    // it to be checked.
                    eprintln!("iod/mqtt: cannot read CA {p}: {e} — not connecting");
                    return;
                }
            },
            // rumqttc builds its root store from this field ALONE — an empty
            // one is a hard error, not a fall back to system roots. So find the
            // system bundle ourselves, and if the image ships none, say which
            // knob fixes it instead of failing with "no valid cert in chain".
            None => match system_roots() {
                Some(b) => b,
                None => {
                    eprintln!("iod/mqtt: mqtts:// requested but no CA bundle found; \
                               set IOD_MQTT_CA to your broker's CA PEM — not connecting");
                    return;
                }
            },
        };
        opts.set_transport(Transport::Tls(TlsConfiguration::Simple {
            ca,
            alpn: None,
            client_auth,
        }));
    }

    // Last will, retained: if this controller drops off, subscribers are told
    // rather than trusting relay states that stopped being updated.
    opts.set_last_will(LastWill::new(format!("{base}/status"), "offline", QoS::AtLeastOnce, true));

    let (client, mut eventloop) = AsyncClient::new(opts, 64);
    let mut rx = cfg.bus.subscribe();

    // Republish the bus outward.
    {
        let client = client.clone();
        let base = base.clone();
        let cfg2 = cfg.clone();
        tokio::task::spawn_local(async move {
            loop {
                match rx.recv().await {
                    Ok(env) => match &env.msg {
                        Msg::State { path, value } => {
                            let t = format!("{base}/state/{path}");
                            let _ = client.publish(t, QoS::AtLeastOnce, true, payload(value)).await;
                        }
                        Msg::Event { topic, data } => {
                            let t = format!("{base}/event/{topic}");
                            let _ = client.publish(t, QoS::AtMostOnce, false, data.to_string()).await;
                        }
                        // The snapshot is only for a newly attached socket; on
                        // MQTT the retained state topics already are it.
                        Msg::Snapshot { .. } => {}
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Republish everything so retained state is correct
                        // again rather than frozen at whatever was missed.
                        for (path, value) in flat(&cfg2.bus.state.doc()) {
                            let t = format!("{base}/state/{path}");
                            let _ = client.publish(t, QoS::AtLeastOnce, true, payload(&value)).await;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
    }

    loop {
        match eventloop.poll().await {
            Ok(MqEvent::Incoming(Incoming::ConnAck(_))) => {
                eprintln!("iod/mqtt: connected to {}:{}", s.host, s.port);
                let _ = client.publish(format!("{base}/status"), QoS::AtLeastOnce, true, "online").await;
                let _ = client.subscribe(format!("{base}/cmd/#"), QoS::AtLeastOnce).await;
                // Retained state, republished on every reconnect: the broker
                // may have been restarted and lost it.
                for (path, value) in flat(&cfg.bus.state.doc()) {
                    let _ = client
                        .publish(format!("{base}/state/{path}"), QoS::AtLeastOnce, true, payload(&value))
                        .await;
                }
                if !s.discovery.is_empty() {
                    announce(&cfg, &client, &s, &base).await;
                }
            }
            Ok(MqEvent::Incoming(Incoming::Publish(p))) => {
                let topic = p.topic.clone();
                let body = String::from_utf8_lossy(&p.payload).to_string();
                if let Some(cmd) = parse_cmd(topic.trim_start_matches(&format!("{base}/cmd/")), &body) {
                    if let Err(e) = ops::dispatch(&cfg, cmd).await {
                        eprintln!("iod/mqtt: {topic}: {e}");
                        let _ = client
                            .publish(format!("{base}/error"), QoS::AtMostOnce, false,
                                     json!({ "topic": topic, "code": e.code(), "message": e.to_string() }).to_string())
                            .await;
                    }
                } else {
                    eprintln!("iod/mqtt: unrecognised command topic {topic}");
                }
            }
            Ok(_) => {}
            Err(e) => {
                // rumqttc reconnects on its own; this is only worth a line so a
                // misconfigured broker is visible in the log.
                eprintln!("iod/mqtt: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Scalars go out bare so `ON`/`OFF`-style consumers work without a JSON
/// parser; anything structured goes as JSON.
fn payload(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Bool(b) => (if *b { "ON" } else { "OFF" }).into(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Flatten the nested state document back to `a/b/c` paths.
fn flat(v: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut Vec<(String, serde_json::Value)>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, val) in m {
                    let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}/{k}") };
                    walk(&p, val, out);
                }
            }
            leaf => out.push((prefix.to_string(), leaf.clone())),
        }
    }
    let mut out = Vec::new();
    walk("", v, &mut out);
    out
}

/// Map a command topic tail plus its payload to a [`Cmd`].
///
/// Payloads are the plain strings an automation tool sends — `ON`, `OFF`,
/// `TOGGLE` — rather than JSON, because half of what will publish here is a
/// Home Assistant switch or a one-line shell script.
fn parse_cmd(tail: &str, body: &str) -> Option<Cmd> {
    let parts: Vec<&str> = tail.split('/').collect();
    let b = body.trim();
    match parts.as_slice() {
        // The escape hatch: anything the control socket accepts, verbatim.
        ["raw"] => serde_json::from_str(b).ok(),
        ["relay", n, "set"] => {
            let index = n.parse().ok()?;
            match b.to_ascii_uppercase().as_str() {
                "ON" | "TRUE" | "1" => Some(Cmd::RelaySet { index, on: true }),
                "OFF" | "FALSE" | "0" => Some(Cmd::RelaySet { index, on: false }),
                "TOGGLE" => Some(Cmd::RelayToggle { index }),
                _ => None,
            }
        }
        ["ir", n, "send"] => Some(Cmd::IrSend { port: n.parse().ok()?, pronto: b.to_string(), repeat: 1 }),
        ["serial", n, "write"] => {
            Some(Cmd::SerialWrite { index: n.parse().ok()?, data: body.to_string(), hex: false, b64: false })
        }
        ["serial", n, "baud"] => Some(Cmd::SerialBaud { index: n.parse().ok()?, baud: b.parse().ok()? }),
        _ => None,
    }
}

/// Home Assistant MQTT discovery.
///
/// The payoff for doing the state/event split properly: the controller appears
/// in HA by itself, with its relays as switches and its contacts as binary
/// sensors, with no YAML written by hand.
async fn announce(cfg: &Arc<Config>, client: &AsyncClient, s: &Settings, base: &str) {
    let id = &s.client_id;
    let device = json!({
        "identifiers": [id],
        "name": cfg.board.hostname,
        "model": cfg.board.model,
        "manufacturer": "openHC",
    });
    let avail = json!([{ "topic": format!("{base}/status") }]);

    for i in 0..cfg.board.io.relays {
        let uid = format!("{id}_relay{i}");
        let cfg_topic = format!("{}/switch/{uid}/config", s.discovery);
        let doc = json!({
            "name": format!("Relay {}", i + 1),
            "unique_id": uid,
            "state_topic": format!("{base}/state/relay/{i}"),
            "command_topic": format!("{base}/cmd/relay/{i}/set"),
            "payload_on": "ON", "payload_off": "OFF",
            "availability": avail, "device": device,
        });
        let _ = client.publish(cfg_topic, QoS::AtLeastOnce, true, doc.to_string()).await;
    }
    for i in 0..cfg.board.io.contacts {
        let uid = format!("{id}_contact{i}");
        let cfg_topic = format!("{}/binary_sensor/{uid}/config", s.discovery);
        let doc = json!({
            "name": format!("Contact {}", i + 1),
            "unique_id": uid,
            "state_topic": format!("{base}/state/contact/{i}"),
            "payload_on": "ON", "payload_off": "OFF",
            "availability": avail, "device": device,
        });
        let _ = client.publish(cfg_topic, QoS::AtLeastOnce, true, doc.to_string()).await;
    }
    eprintln!(
        "iod/mqtt: announced {} relays and {} contacts to Home Assistant",
        cfg.board.io.relays, cfg.board.io.contacts
    );
}
