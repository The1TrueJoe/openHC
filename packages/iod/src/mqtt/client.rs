//! The outbound bridge — iod as a CLIENT of somebody else's broker.
//!
//! Optional, and independent of the served endpoint. A controller that runs
//! standalone never starts this; one that belongs to a house running Home
//! Assistant or Node-RED publishes into that broker as well, so the same topics
//! appear in both places.
//!
//! The mapping is [`super::topics`], shared with the served endpoint so the two
//! cannot drift. It leans on the distinction MQTT cares about:
//!
//! * **state** → `<prefix>/<id>/state/<path>`, RETAINED. A subscriber that
//!   connects an hour late is immediately told every relay and contact.
//! * **events** → `<prefix>/<id>/event/<topic>`, NOT retained. Replaying "a
//!   remote was pressed" to whoever connects next would be a lie.
//!
//! Getting that backwards is the classic MQTT integration bug: retained events
//! re-fire automations on every reconnect, and unretained state leaves a
//! restarted consumer blind until something happens to change.
use super::settings::Mqtt;
use super::topics;
use crate::ops;
use crate::Config;
use rumqttc::{AsyncClient, Event as MqEvent, Incoming, LastWill, MqttOptions, QoS, TlsConfiguration, Transport};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The connection parameters, resolved from settings.
struct Cfg {
    host: String,
    port: u16,
    tls: bool,
    user: Option<String>,
    pass: Option<String>,
    ca: Option<String>,
    client_cert: Option<String>,
    client_key: Option<String>,
    prefix: String,
    client_id: String,
    discovery: String,
}

fn opt(s: &str) -> Option<String> {
    Some(s).filter(|v| !v.is_empty()).map(str::to_string)
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

/// Split `mqtt://user@host:1883` into host, port and whether it is TLS.
fn split_url(url: &str) -> (String, u16, bool) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("mqtt", url));
    let tls = scheme.eq_ignore_ascii_case("mqtts") || scheme.eq_ignore_ascii_case("ssl");
    let rest = rest.trim_end_matches('/');
    match rest.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(if tls { 8883 } else { 1883 }), tls),
        None => (rest.to_string(), if tls { 8883 } else { 1883 }, tls),
    }
}

/// Run the bridge until cancelled, reconnecting on its own.
pub async fn run(cfg: Arc<Config>, m: Mqtt) {
    let (host, port, tls) = split_url(&m.url);
    if host.is_empty() {
        eprintln!("iod/mqtt: bridge enabled but no broker URL set");
        return;
    }
    let s = Cfg {
        host, port, tls,
        user: opt(&m.username), pass: opt(&m.password),
        ca: opt(&m.ca_path),
        client_cert: opt(&m.client_cert_path), client_key: opt(&m.client_key_path),
        prefix: m.prefix.clone(), client_id: m.client_id.clone(), discovery: m.discovery.clone(),
    };
    if s.tls {
        // rustls is built here with no default crypto provider (see Cargo.toml
        // — the default one needs a C toolchain this workspace does not have),
        // so the provider has to be chosen explicitly before any TLS handshake.
        // Err just means something already installed one.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    }
    let base = topics::base(&s.prefix, &s.client_id);
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
                    Ok(env) => {
                        if let Some((topic, body, retain)) = topics::route(&base, &env.msg) {
                            let qos = if retain { QoS::AtLeastOnce } else { QoS::AtMostOnce };
                            let _ = client.publish(topic, qos, retain, body).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Republish everything so retained state is correct
                        // again rather than frozen at whatever was missed.
                        for (topic, body) in topics::retained(&base, &cfg2.bus.state.doc()) {
                            let _ = client.publish(topic, QoS::AtLeastOnce, true, body).await;
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
                for (topic, body) in topics::retained(&base, &cfg.bus.state.doc()) {
                    let _ = client.publish(topic, QoS::AtLeastOnce, true, body).await;
                }
                if !s.discovery.is_empty() {
                    announce(&cfg, &client, &s, &base).await;
                }
            }
            Ok(MqEvent::Incoming(Incoming::Publish(p))) => {
                let topic = p.topic.clone();
                let body = String::from_utf8_lossy(&p.payload).to_string();
                if let Some(cmd) = super::parse_cmd(topic.trim_start_matches(&format!("{base}/cmd/")), &body) {
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

/// Home Assistant MQTT discovery.
///
/// The payoff for doing the state/event split properly: the controller appears
/// in HA by itself, with its relays as switches and its contacts as binary
/// sensors, with no YAML written by hand.
async fn announce(cfg: &Arc<Config>, client: &AsyncClient, s: &Cfg, base: &str) {
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
