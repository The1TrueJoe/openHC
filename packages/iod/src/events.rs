//! The control plane: state that is mirrored, events that merely happen.
//!
//! These are genuinely different things and conflating them makes a bad API.
//!
//! **State** is what the box currently IS — relay 2 is closed, contact 1 is
//! made, the MCU link is up. It has a value at every instant, a new client must
//! be told it on connect, and a client that misses a change is WRONG until the
//! next one. So state is mirrored in [`State`], sent as a snapshot to every new
//! subscriber, and published as a delta whenever it changes — no matter who
//! changed it. Two people on the config GUI see the same relay, and an external
//! control system can hold an accurate mirror without polling.
//!
//! **Events** are things that HAPPENED — a byte arrived on a serial port, a
//! remote was pressed at the IR receiver. They have no value between
//! occurrences, snapshotting them is meaningless, and a client that connects
//! late has simply missed them. These stream, and are never retained.
//!
//! The split is not academic: it is exactly MQTT's retained-vs-not, exactly
//! what decides whether a reconnecting automation is correct or stale, and
//! exactly the difference between "the door is open" and "the door opened".
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Every message carries a sequence number and a timestamp.
///
/// `seq` is monotonic across the whole stream so a consumer can prove it missed
/// nothing — a control system acting on stale IO is worse than one that knows
/// it fell behind and resynchronises.
#[derive(Serialize, Clone, Debug)]
pub struct Envelope {
    pub seq: u64,
    /// Unix seconds, fractional. Wall clock rather than uptime because the
    /// consumer correlating this with its own logs is on another machine.
    pub ts: f64,
    #[serde(flatten)]
    pub msg: Msg,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    /// The whole state document. Sent on connect, and on request.
    Snapshot { state: Value },
    /// One piece of state changed. `path` is the same key used in the snapshot.
    State { path: String, value: Value },
    /// Something happened. No value, no retention.
    Event { topic: String, data: Value },
}

impl Msg {
    /// The routing key, for subscription filters and MQTT topic mapping.
    /// Prefix matching on this is how a client says "contacts and IR, but not
    /// the byte stream of a chatty projector".
    pub fn topic(&self) -> &str {
        match self {
            Msg::Snapshot { .. } => "snapshot",
            Msg::State { path, .. } => path,
            Msg::Event { topic, .. } => topic,
        }
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The authoritative mirror of everything that HAS a current value.
///
/// Held here rather than read from the MCU on demand because the MCU is one
/// UART: five clients asking "what are the relays" must not become five
/// round-trips behind a mutex. Writers update this, readers read it for free.
#[derive(Default)]
pub struct State {
    inner: Mutex<BTreeMap<String, Value>>,
}

impl State {
    pub fn get(&self, path: &str) -> Option<Value> {
        self.inner.lock().ok()?.get(path).cloned()
    }

    /// The whole document, as nested JSON: `relay/1` becomes `{"relay":{"1":…}}`
    /// so a client can hold it as one object.
    pub fn doc(&self) -> Value {
        let map = match self.inner.lock() {
            Ok(m) => m,
            Err(_) => return json!({}),
        };
        let mut root = serde_json::Map::new();
        for (k, v) in map.iter() {
            let mut cur = &mut root;
            let parts: Vec<&str> = k.split('/').collect();
            for p in &parts[..parts.len() - 1] {
                cur = cur
                    .entry(*p)
                    .or_insert_with(|| Value::Object(serde_json::Map::new()))
                    .as_object_mut()
                    .expect("state paths do not collide with leaf values");
            }
            cur.insert(parts[parts.len() - 1].to_string(), v.clone());
        }
        Value::Object(root)
    }
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Envelope>,
    /// A plain mutex, NOT an atomic, because the IO Extender's ARM926EJ-S is
    /// ARMv5TE and has no 64-bit atomics — `std::sync::atomic::AtomicU64` does
    /// not exist on that target, so iod simply did not compile for it.
    /// Narrowing to `AtomicUsize` was the other option and is worse: usize is
    /// 32 bits there, so the counter would silently wrap, and a wrap in a
    /// gap-detection sequence looks exactly like the gap it exists to detect.
    /// The lock costs nothing — iod runs on a current-thread runtime, so it is
    /// never contended.
    seq: Arc<Mutex<u64>>,
    pub state: Arc<State>,
}

impl Bus {
    pub fn new() -> Bus {
        // Bounded on purpose. A client that stops reading must not grow this
        // without limit; broadcast drops the oldest for that receiver and tells
        // it how many it missed, which is the right failure for a live view.
        // 512 is generous enough to absorb a burst of serial traffic without
        // lagging a slow browser off the bus.
        let (tx, _) = broadcast::channel(512);
        Bus { tx, seq: Arc::new(Mutex::new(0)), state: Arc::new(State::default()) }
    }

    fn send(&self, msg: Msg) {
        // Pre-increment, so the counter always holds the LAST seq assigned and
        // the first real message is 1. `snapshot` depends on that: it must be
        // able to name a position without consuming one.
        let seq = {
            let Ok(mut n) = self.seq.lock() else { return };
            *n += 1;
            *n
        };
        // Err just means nobody is listening.
        let _ = self.tx.send(Envelope { seq, ts: now(), msg });
    }

    /// Record a state value and publish a delta — but ONLY if it actually
    /// changed. The contact poller runs five times a second; without this
    /// check every subscriber would receive four identical "contact 0 is open"
    /// messages a second and a delta would stop meaning anything.
    pub fn set(&self, path: &str, value: Value) {
        {
            let Ok(mut m) = self.state.inner.lock() else { return };
            if m.get(path) == Some(&value) {
                return;
            }
            m.insert(path.to_string(), value.clone());
        }
        self.send(Msg::State { path: path.to_string(), value });
    }

    /// Publish something that happened. Never retained, never deduplicated —
    /// two identical remote presses are two events.
    pub fn event(&self, topic: &str, data: Value) {
        self.send(Msg::Event { topic: topic.to_string(), data });
    }

    /// The current state document, for a client that just arrived.
    ///
    /// Its `seq` is the last sequence number REFLECTED in the document, not a
    /// new one. A snapshot goes to a single socket, so consuming a global
    /// sequence number would punch a gap in every other client's stream — and a
    /// gap is exactly what those numbers exist to make detectable. A client can
    /// therefore expect strictly increasing seq across the snapshot and
    /// everything after it; 0 means nothing has been published yet.
    pub fn snapshot(&self) -> Envelope {
        Envelope {
            seq: self.seq.lock().map(|n| *n).unwrap_or(0),
            ts: now(),
            msg: Msg::Snapshot { state: self.state.doc() },
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_deltas_only_fire_on_change() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        bus.set("contact/0", serde_json::json!(false));
        // The poller writes this five times a second; only a real transition
        // should reach a subscriber.
        bus.set("contact/0", serde_json::json!(false));
        bus.set("contact/0", serde_json::json!(true));
        drop(bus);

        let mut seen = Vec::new();
        while let Ok(e) = rx.try_recv() {
            if let Msg::State { path, value } = e.msg {
                seen.push((path, value));
            }
        }
        assert_eq!(seen.len(), 2, "duplicate writes must not publish: {seen:?}");
    }

    #[test]
    fn snapshot_seq_does_not_collide_with_the_next_message() {
        let bus = Bus::new();
        // Nothing published: the snapshot names position 0, and the first real
        // message must be 1 so a client sees a strictly increasing stream.
        assert_eq!(bus.snapshot().seq, 0);
        let mut rx = bus.subscribe();
        bus.set("relay/0", serde_json::json!(true));
        assert_eq!(rx.try_recv().unwrap().seq, 1);
        // And a snapshot taken now reports the last seq it reflects, not the
        // next one to be handed out.
        assert_eq!(bus.snapshot().seq, 1);
        bus.set("relay/0", serde_json::json!(false));
        assert_eq!(rx.try_recv().unwrap().seq, 2);
    }

    #[test]
    fn doc_nests_paths() {
        let bus = Bus::new();
        bus.set("relay/0", serde_json::json!(true));
        bus.set("serial/1/baud", serde_json::json!(9600));
        assert_eq!(bus.state.doc(), serde_json::json!({
            "relay": { "0": true },
            "serial": { "1": { "baud": 9600 } },
        }));
    }
}
