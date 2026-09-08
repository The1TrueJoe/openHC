//! MQTT settings — the part the config GUI writes.
//!
//! Persisted as JSON at `/etc/openhc/iod.json` rather than appended to the
//! shell-style `iod.conf`, because this file is now written by a MACHINE. A
//! daemon rewriting a commented shell file either destroys the comments or
//! needs a shell parser to preserve them; JSON round-trips honestly.
//!
//! `iod.conf` still works and still wins: environment variables override the
//! file. Someone who configures a fleet by dropping in a conf file should not
//! find the GUI silently disagreeing with it, and an operator staring at a
//! provisioned value they cannot change from the UI is better served by seeing
//! it marked as pinned than by having their edit quietly ignored.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    std::env::var("IOD_SETTINGS")
        .unwrap_or_else(|_| "/etc/openhc/iod.json".into())
        .into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Mqtt {
    /// Serve MQTT from this controller. The config GUI talks to it over a
    /// WebSocket, so turning this off leaves the IO panels with nothing to
    /// speak to — which is why it defaults on and the UI says as much.
    pub serve: bool,
    /// Plain-MQTT listener for other things on the LAN. 0 disables it; the
    /// WebSocket endpoint is always available through webd regardless.
    pub listen_port: u16,
    /// Also connect OUT to somebody else's broker. Independent of `serve`:
    /// the GUI keeps talking to this controller either way, so pointing at a
    /// house broker never costs you the ability to configure the box.
    pub bridge: bool,
    pub url: String,
    pub username: String,
    pub password: String,
    pub ca_path: String,
    pub client_cert_path: String,
    pub client_key_path: String,
    /// Topic root. `<prefix>/<client_id>/…`
    pub prefix: String,
    /// Defaults to the hostname, which already carries the MAC and is unique.
    pub client_id: String,
    /// Home Assistant discovery prefix; empty disables it.
    pub discovery: String,
}

impl Default for Mqtt {
    fn default() -> Self {
        Mqtt {
            serve: true,
            listen_port: 1883,
            bridge: false,
            url: String::new(),
            username: String::new(),
            password: String::new(),
            ca_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            prefix: "openhc".into(),
            client_id: String::new(),
            discovery: "homeassistant".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Settings {
    pub mqtt: Mqtt,
}

impl Settings {
    /// File, then environment on top. A missing file is not an error — it is
    /// what an unconfigured controller looks like.
    pub fn load(hostname: &str) -> (Settings, Vec<String>) {
        let mut s: Settings = std::fs::read_to_string(path())
            .ok()
            .and_then(|t| match serde_json::from_str(&t) {
                Ok(v) => Some(v),
                Err(e) => {
                    // Loud, and then defaults. Silently starting with an empty
                    // config because of a stray comma would look like the
                    // settings had been wiped.
                    eprintln!("iod: {} is not valid JSON ({e}); using defaults", path().display());
                    None
                }
            })
            .unwrap_or_default();

        // Which fields the environment has pinned, so the UI can show them as
        // read-only instead of accepting an edit it cannot honour.
        let mut pinned = Vec::new();
        let mut env_str = |key: &str, field: &mut String, name: &str| {
            if let Ok(v) = std::env::var(key) {
                if !v.is_empty() {
                    *field = v;
                    pinned.push(name.to_string());
                }
            }
        };
        env_str("IOD_MQTT_URL", &mut s.mqtt.url, "url");
        env_str("IOD_MQTT_USER", &mut s.mqtt.username, "username");
        env_str("IOD_MQTT_PASS", &mut s.mqtt.password, "password");
        env_str("IOD_MQTT_CA", &mut s.mqtt.ca_path, "ca_path");
        env_str("IOD_MQTT_CLIENT_CERT", &mut s.mqtt.client_cert_path, "client_cert_path");
        env_str("IOD_MQTT_CLIENT_KEY", &mut s.mqtt.client_key_path, "client_key_path");
        env_str("IOD_MQTT_PREFIX", &mut s.mqtt.prefix, "prefix");
        env_str("IOD_MQTT_CLIENT_ID", &mut s.mqtt.client_id, "client_id");
        if let Ok(v) = std::env::var("IOD_MQTT_DISCOVERY") {
            s.mqtt.discovery = v;
            pinned.push("discovery".into());
        }
        // A URL in the environment is how the pre-settings builds were
        // configured; honour it as "bridge on" rather than ignoring it.
        if !s.mqtt.url.is_empty() && std::env::var("IOD_MQTT_URL").is_ok() {
            s.mqtt.bridge = true;
            pinned.push("bridge".into());
        }

        if s.mqtt.client_id.is_empty() {
            s.mqtt.client_id = hostname.to_string();
        }
        if s.mqtt.prefix.is_empty() {
            s.mqtt.prefix = "openhc".into();
        }
        (s, pinned)
    }

    /// Write atomically: a controller that loses power mid-save should come
    /// back with the old settings, not half of the new ones.
    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &p)
    }

    pub fn exists() -> bool {
        Path::new(&path()).exists()
    }
}
