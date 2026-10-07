//! The topic map — one definition, used by both the served endpoint and the
//! outbound bridge.
//!
//! If these two disagreed, a controller talking to Home Assistant would publish
//! different topics than the same controller talking to its own config GUI,
//! and every automation written against one would break against the other.
use crate::events::Msg;
use serde_json::Value;

/// Turn a zero-based internal index into the number on the panel.
///
/// Relays, contacts, IR ports and serial ports are all labelled from 1 on the
/// hardware, so that is what the topics carry: `relay/1` is the terminal marked
/// 1. Internally everything counts from zero — a gpiochip offset and the MCU's
///    wire selector both must — and the two edges that convert are this module
///    and [`crate::gpio_io`].
pub fn label(index: usize) -> usize {
    index + 1
}

/// The inverse, for a number arriving from a client. `None` for 0, which is not
/// a label any hardware carries and is far more likely to be a client that
/// assumed zero-based.
pub fn index(label: &str) -> Option<u8> {
    match label.parse::<u8>() {
        Ok(n) if n >= 1 => Some(n - 1),
        _ => None,
    }
}

/// `<prefix>/<id>` — everything hangs off this.
pub fn base(prefix: &str, id: &str) -> String {
    format!("{prefix}/{id}")
}

/// Where a bus message goes, and whether it is retained.
///
/// The retain flag is the whole reason state and events are separate types:
/// retained state means a subscriber that connects an hour late is told the
/// truth immediately, and NOT retaining events means "a remote was pressed" is
/// never replayed to whoever connects next.
pub fn route(base: &str, msg: &Msg) -> Option<(String, String, bool)> {
    match msg {
        Msg::State { path, value } => Some((format!("{base}/state/{path}"), payload(value), true)),
        Msg::Event { topic, data } => Some((format!("{base}/event/{topic}"), data.to_string(), false)),
        // Only meaningful to a socket that just attached; on MQTT the retained
        // state topics already are the snapshot.
        Msg::Snapshot { .. } => None,
    }
}

/// Scalars go out bare so `ON`/`OFF` consumers work without a JSON parser;
/// anything structured goes as JSON.
pub fn payload(v: &Value) -> String {
    match v {
        Value::Bool(b) => (if *b { "ON" } else { "OFF" }).into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Every retained topic, for a fresh subscriber or a reconnected broker.
///
/// One topic per path the state was SET at, with the same payload [`route`]
/// gives a live change — never a flattening of the nested document. Flattening
/// split a structured value (`system/restore` = `{available, detail, ...}`) into
/// leaf topics on reconnect while live changes published it whole, so the same
/// state had two shapes depending on timing, and the leaves went stale the
/// moment the value's shape changed.
pub fn retained(base: &str, entries: &[(String, Value)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(p, v)| (format!("{base}/state/{p}"), payload(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_go_out_as_on_off() {
        assert_eq!(payload(&serde_json::json!(true)), "ON");
        assert_eq!(payload(&serde_json::json!(false)), "OFF");
        assert_eq!(payload(&serde_json::json!(115200)), "115200");
    }

    #[test]
    fn retained_matches_live_routing() {
        // A structured value stays ONE topic, exactly as `route` publishes it.
        let restore = serde_json::json!({"available": true, "openhc": true});
        let entries = vec![
            ("relay/1".to_string(), serde_json::json!(true)),
            ("serial/1/baud".to_string(), serde_json::json!(9600)),
            ("system/restore".to_string(), restore.clone()),
        ];
        let mut got = retained("openhc/box", &entries);
        got.sort();
        assert_eq!(got, vec![
            ("openhc/box/state/relay/1".into(), "ON".into()),
            ("openhc/box/state/serial/1/baud".into(), "9600".into()),
            ("openhc/box/state/system/restore".into(), restore.to_string()),
        ]);
        let live = route("openhc/box", &Msg::State { path: "system/restore".into(), value: restore }).unwrap();
        assert_eq!((live.0, live.1), got[2].clone());
    }
}
