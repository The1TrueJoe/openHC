//! iod as a client of the box's mosquitto.
//!
//! Every board runs mosquitto, and every openHC daemon is a client of it with
//! its own topics under one tree (`<prefix>/<host>/`): iod the IO, ohc-audiod
//! the audio, sysmond the health. Reaching a house broker is mosquitto's own
//! bridge (see bridge.rs), so this client only ever talks to loopback — no TLS,
//! no credentials, no reconnect policy beyond rumqttc's own.
//!
//! The mapping is [`super::topics`]. It leans on the distinction MQTT cares
//! about:
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
use rumqttc::{AsyncClient, Event as MqEvent, Incoming, LastWill, MqttOptions, QoS};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// Run until cancelled; rumqttc reconnects on its own.
pub async fn run(cfg: Arc<Config>, m: Mqtt) {
    let (host, port) = ohcmqtt::broker();
    let base = topics::base(&m.prefix, &m.client_id);
    // Its own client id on the shared broker: `<host>-iod`, beside
    // `<host>-audiod` and `<host>-sysmond`.
    let mut opts = MqttOptions::new(format!("{}-iod", m.client_id), &host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    // Last will, retained: if iod drops off, subscribers are told rather than
    // trusting relay states that stopped being updated.
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
                        for (topic, body) in topics::retained(&base, &cfg2.bus.state.entries()) {
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
                eprintln!("iod/mqtt: connected to {host}:{port} under {base}");
                let _ = client.publish(format!("{base}/status"), QoS::AtLeastOnce, true, "online").await;
                let _ = client.subscribe(format!("{base}/cmd/#"), QoS::AtLeastOnce).await;
                // Retained state, republished on every reconnect: the broker
                // keeps it in RAM and may have restarted.
                for (topic, body) in topics::retained(&base, &cfg.bus.state.entries()) {
                    let _ = client.publish(topic, QoS::AtLeastOnce, true, body).await;
                }
                if !m.discovery.is_empty() {
                    announce(&cfg, &client, &m, &base).await;
                }
            }
            Ok(MqEvent::Incoming(Incoming::Publish(p))) => {
                let topic = p.topic.clone();
                let tail = topic.trim_start_matches(&format!("{base}/cmd/")).to_string();
                // Other daemons own their own command subtrees on the same
                // broker (cmd/audio/* is ohc-audiod's, cmd/health/* sysmond's).
                if tail.starts_with("audio/") || tail.starts_with("health/") {
                    continue;
                }
                let body = String::from_utf8_lossy(&p.payload).to_string();
                if let Some(cmd) = super::parse_cmd(&tail, &body) {
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
                eprintln!("iod/mqtt: {e}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

/// Home Assistant MQTT discovery.
///
/// The payoff for doing the state/event split properly: the controller appears
/// in HA by itself, with its relays as switches and its contacts as binary
/// sensors, with no YAML written by hand. Topics use the PANEL number (relay 1
/// is `relay/1`), the same identity iod publishes and parses — announcing the
/// zero-based index here pointed every entity one relay off.
async fn announce(cfg: &Arc<Config>, client: &AsyncClient, m: &Mqtt, base: &str) {
    let id = &m.client_id;
    let device = json!({
        "identifiers": [id],
        "name": cfg.board.hostname,
        "model": cfg.board.model,
        "manufacturer": "openHC",
    });
    let avail = json!([{ "topic": format!("{base}/status") }]);

    for i in 0..cfg.board.io.relays {
        let n = topics::label(i as usize);
        let uid = format!("{id}_relay{n}");
        let doc = json!({
            "name": format!("Relay {n}"),
            "unique_id": uid,
            "state_topic": format!("{base}/state/relay/{n}"),
            "command_topic": format!("{base}/cmd/relay/{n}/set"),
            "payload_on": "ON", "payload_off": "OFF",
            "availability": avail, "device": device,
        });
        let _ = client
            .publish(format!("{}/switch/{uid}/config", m.discovery), QoS::AtLeastOnce, true, doc.to_string())
            .await;
    }
    for i in 0..cfg.board.io.contacts {
        let n = topics::label(i as usize);
        let uid = format!("{id}_contact{n}");
        let doc = json!({
            "name": format!("Contact {n}"),
            "unique_id": uid,
            "state_topic": format!("{base}/state/contact/{n}"),
            "payload_on": "ON", "payload_off": "OFF",
            "availability": avail, "device": device,
        });
        let _ = client
            .publish(format!("{}/binary_sensor/{uid}/config", m.discovery), QoS::AtLeastOnce, true, doc.to_string())
            .await;
    }
    eprintln!(
        "iod/mqtt: announced {} relays and {} contacts to Home Assistant",
        cfg.board.io.relays, cfg.board.io.contacts
    );
}
