//! iod serving MQTT — to the config GUI over a WebSocket, and to anything else
//! on the LAN over plain TCP.
//!
//! This is NOT a general broker. It carries exactly one device's topics, has no
//! persistence, no cross-client routing beyond iod's own publications, and no
//! QoS 2. That narrowness is the point: a real broker was 157 extra crates
//! including a YAML parser, a CLI argument parser and a second HTTP stack, for
//! a controller whose entire topic space is four relays and a couple of UARTs.
//!
//! The MQTT codec comes from rumqttc, which iod already uses as a client. So
//! speaking the protocol in both directions costs no new dependency, and the
//! wire format the GUI sees is byte-for-byte the one an external broker sees.
use super::{settings::Mqtt, topics};
use crate::ops;
use crate::Config;
use bytes::BytesMut;
use rumqttc::{
    ConnAck, ConnectReturnCode, Packet, PubAck, Publish, QoS, SubAck, SubscribeFilter,
    SubscribeReasonCode, UnsubAck,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Cap a single packet. A browser has no business sending iod a megabyte, and
/// an unbounded read is a way to be killed by one client.
const MAX_PACKET: usize = 256 * 1024;

/// One connected client, transport-agnostic.
///
/// The transport is reduced to two channels before it gets here, so the
/// protocol logic is written once and a WebSocket and a TCP socket are the
/// same thing to it.
pub async fn session(
    cfg: Arc<Config>,
    mqtt: Mqtt,
    mut inbound: mpsc::Receiver<Packet>,
    outbound: mpsc::Sender<Packet>,
    peer: String,
) {
    let base = topics::base(&mqtt.prefix, &mqtt.client_id);

    // CONNECT must come first, and nothing else is answered until it does.
    let Some(Packet::Connect(c)) = inbound.recv().await else {
        return;
    };

    // The API token doubles as the MQTT password. One secret for the box, not
    // one per protocol — an operator who set IOD_TOKEN has said what they mean.
    if let Ok(want) = std::env::var("IOD_TOKEN") {
        if !want.is_empty() {
            let got = c.login.as_ref().map(|l| l.password.clone()).unwrap_or_default();
            if !crate::api::constant_eq(got.as_bytes(), want.as_bytes()) {
                eprintln!("iod/mqttd: {peer} rejected — bad password");
                let _ = outbound
                    .send(Packet::ConnAck(ConnAck::new(ConnectReturnCode::BadUserNamePassword, false)))
                    .await;
                return;
            }
        }
    }
    eprintln!("iod/mqttd: {peer} connected as {:?}", c.client_id);
    if outbound.send(Packet::ConnAck(ConnAck::new(ConnectReturnCode::Success, false))).await.is_err() {
        return;
    }

    let mut bus = cfg.bus.subscribe();
    let mut filters: Vec<SubscribeFilter> = Vec::new();

    loop {
        tokio::select! {
            pkt = inbound.recv() => {
                let Some(pkt) = pkt else { break };
                match pkt {
                    Packet::Subscribe(s) => {
                        let codes = s.filters.iter()
                            // Max QoS 1: this endpoint does not implement the
                            // QoS 2 handshake, and granting a level we cannot
                            // honour is worse than granting a lower one.
                            .map(|f| SubscribeReasonCode::Success(match f.qos {
                                QoS::ExactlyOnce => QoS::AtLeastOnce,
                                q => q,
                            }))
                            .collect();
                        if outbound.send(Packet::SubAck(SubAck::new(s.pkid, codes))).await.is_err() { break }
                        // Retained state, immediately. This is what makes a
                        // freshly-loaded page show the truth instead of blanks
                        // until something happens to change.
                        for f in &s.filters {
                            if !send_retained(&cfg, &base, &f.path, &outbound).await { return }
                        }
                        filters.extend(s.filters);
                    }
                    Packet::Unsubscribe(u) => {
                        filters.retain(|f| !u.topics.contains(&f.path));
                        if outbound.send(Packet::UnsubAck(UnsubAck::new(u.pkid))).await.is_err() { break }
                    }
                    Packet::Publish(p) => {
                        if p.qos == QoS::AtLeastOnce
                            && outbound.send(Packet::PubAck(PubAck::new(p.pkid))).await.is_err() { break }
                        handle_command(&cfg, &base, &p, &outbound).await;
                    }
                    Packet::PingReq => {
                        if outbound.send(Packet::PingResp).await.is_err() { break }
                    }
                    Packet::Disconnect => break,
                    _ => {}
                }
            }
            ev = bus.recv() => match ev {
                Ok(env) => {
                    let Some((topic, payload, retain)) = topics::route(&base, &env.msg) else { continue };
                    if !filters.iter().any(|f| topics::matches(&f.path, &topic)) { continue }
                    let mut pub_ = Publish::new(topic, QoS::AtMostOnce, payload);
                    pub_.retain = retain;
                    if outbound.send(Packet::Publish(pub_)).await.is_err() { break }
                }
                // This client could not keep up. Its view of state is now a
                // guess, so replay everything retained rather than leaving it
                // subtly wrong — the same reasoning as a broker reconnect.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("iod/mqttd: {peer} lagged {n}; resending retained state");
                    for f in filters.clone() {
                        if !send_retained(&cfg, &base, &f.path, &outbound).await { return }
                    }
                }
                Err(_) => break,
            },
        }
    }
    eprintln!("iod/mqttd: {peer} disconnected");
}

/// Publish every retained topic matching `filter`.
async fn send_retained(
    cfg: &Arc<Config>,
    base: &str,
    filter: &str,
    out: &mpsc::Sender<Packet>,
) -> bool {
    let mut items = topics::retained(base, &cfg.bus.state.doc());
    // Served by this process, so it is online by definition — but a client
    // subscribing to `status` still expects to be told.
    items.push((format!("{base}/status"), "online".into()));
    for (topic, payload) in items {
        if !topics::matches(filter, &topic) {
            continue;
        }
        let mut p = Publish::new(topic, QoS::AtMostOnce, payload);
        p.retain = true;
        if out.send(Packet::Publish(p)).await.is_err() {
            return false;
        }
    }
    true
}

/// A client publication. Only `<base>/cmd/...` means anything.
async fn handle_command(cfg: &Arc<Config>, base: &str, p: &Publish, out: &mpsc::Sender<Packet>) {
    let prefix = format!("{base}/cmd/");
    let Some(tail) = p.topic.strip_prefix(&prefix) else {
        // Anything else is a client publishing into our namespace. Ignoring it
        // beats echoing it: this is a device endpoint, not a message bus.
        return;
    };
    let body = String::from_utf8_lossy(&p.payload).to_string();
    let Some(cmd) = super::parse_cmd(tail, &body) else {
        eprintln!("iod/mqttd: unrecognised command topic {}", p.topic);
        return;
    };
    if let Err(e) = ops::dispatch(cfg, cmd).await {
        eprintln!("iod/mqttd: {}: {e}", p.topic);
        let err = serde_json::json!({ "topic": p.topic, "code": e.code(), "message": e.to_string() });
        let _ = out
            .send(Packet::Publish(Publish::new(format!("{base}/error"), QoS::AtMostOnce, err.to_string())))
            .await;
    }
}

// ── transports ─────────────────────────────────────────────────────────────

/// Decode a byte stream into packets. Shared by both transports because MQTT
/// framing does not care what carried the bytes.
pub struct Framer {
    buf: BytesMut,
}

impl Framer {
    pub fn new() -> Framer {
        Framer { buf: BytesMut::with_capacity(4096) }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    /// Next complete packet, if one has arrived.
    pub fn next(&mut self) -> Result<Option<Packet>, rumqttc::mqttbytes::Error> {
        match Packet::read(&mut self.buf, MAX_PACKET) {
            Ok(p) => Ok(Some(p)),
            Err(rumqttc::mqttbytes::Error::InsufficientBytes(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub fn encode(p: &Packet) -> Option<Vec<u8>> {
    let mut b = BytesMut::new();
    p.write(&mut b, MAX_PACKET).ok()?;
    Some(b.to_vec())
}

/// Plain-MQTT listener, for things on the LAN that are not a browser.
pub async fn listen_tcp(cfg: Arc<Config>, mqtt: Mqtt, port: u16) {
    let addr = format!("0.0.0.0:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("iod/mqttd: cannot bind {addr}: {e}");
            return;
        }
    };
    eprintln!("iod/mqttd: listening on {addr}");
    loop {
        let Ok((sock, peer)) = listener.accept().await else { continue };
        let (cfg, mqtt) = (cfg.clone(), mqtt.clone());
        tokio::spawn(async move {
            let (mut rd, mut wr) = sock.into_split();
            let (in_tx, in_rx) = mpsc::channel::<Packet>(64);
            let (out_tx, mut out_rx) = mpsc::channel::<Packet>(256);

            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(p) = out_rx.recv().await {
                    let Some(b) = encode(&p) else { continue };
                    if wr.write_all(&b).await.is_err() { return }
                }
            });
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut framer = Framer::new();
                let mut buf = [0u8; 4096];
                loop {
                    let Ok(n) = rd.read(&mut buf).await else { return };
                    if n == 0 { return }
                    framer.feed(&buf[..n]);
                    loop {
                        match framer.next() {
                            Ok(Some(p)) => { if in_tx.send(p).await.is_err() { return } }
                            Ok(None) => break,
                            Err(_) => return,
                        }
                    }
                }
            });
            session(cfg, mqtt, in_rx, out_tx, peer.to_string()).await;
        });
    }
}
