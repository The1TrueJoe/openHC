//! Bridging to a house broker — mosquitto's job, configured from iod's settings.
//!
//! The settings page has always had "also publish to an external broker"
//! (URL, credentials, CA, client cert). With a real broker on the box, that is
//! exactly mosquitto's bridge feature, and doing it there carries EVERY daemon's
//! topics (iod's IO, ohc-audiod's audio, sysmond's health), not just iod's. So
//! iod renders the stanza into `/etc/mosquitto/conf.d/openhc-bridge.conf` and
//! restarts mosquitto when it changed; an empty/disabled bridge removes the file.
//!
//! The whole `<prefix>/<host>/#` tree is bridged BOTH ways (state out, commands
//! in from the house), and the Home Assistant discovery prefix out.
use super::settings::Mqtt;

fn conf_path() -> String {
    std::env::var("IOD_BRIDGE_CONF").unwrap_or_else(|_| "/etc/mosquitto/conf.d/openhc-bridge.conf".into())
}

/// `mqtt://host:port`, `mqtts://…` (or `ssl://`) → (host, port, tls).
pub fn split_url(url: &str) -> (String, u16, bool) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("mqtt", url));
    let tls = scheme.eq_ignore_ascii_case("mqtts") || scheme.eq_ignore_ascii_case("ssl");
    let rest = rest.trim_end_matches('/');
    let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
    match rest.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(if tls { 8883 } else { 1883 }), tls),
        None => (rest.to_string(), if tls { 8883 } else { 1883 }, tls),
    }
}

/// The bridge stanza for these settings, or `None` when no bridge is wanted.
pub fn render(m: &Mqtt) -> Option<String> {
    if !m.bridge || m.url.trim().is_empty() {
        return None;
    }
    let (host, port, tls) = split_url(m.url.trim());
    if host.is_empty() {
        return None;
    }
    let base = format!("{}/{}", m.prefix, m.client_id);
    let mut s = format!(
        "# Written by iod from its MQTT settings — edit those, not this file.\n\
         connection openhc-bridge\n\
         address {host}:{port}\n\
         remote_clientid {id}\n\
         bridge_protocol_version mqttv311\n\
         try_private false\n\
         cleansession true\n\
         notifications true\n\
         notification_topic {base}/status/bridge\n\
         topic {base}/# both 1\n",
        id = m.client_id,
    );
    if !m.discovery.is_empty() {
        s += &format!("topic {}/# out 1\n", m.discovery);
    }
    if !m.username.is_empty() {
        s += &format!("remote_username {}\n", m.username);
    }
    if !m.password.is_empty() {
        s += &format!("remote_password {}\n", m.password);
    }
    if tls {
        if m.ca_path.is_empty() {
            // A publicly-issued certificate: the system bundle.
            s += "bridge_capath /etc/ssl/certs\n";
        } else {
            s += &format!("bridge_cafile {}\n", m.ca_path);
        }
        if !m.client_cert_path.is_empty() && !m.client_key_path.is_empty() {
            s += &format!("bridge_certfile {}\nbridge_keyfile {}\n", m.client_cert_path, m.client_key_path);
        }
    }
    Some(s)
}

/// Bring mosquitto's bridge in line with the settings. Restarts mosquitto only
/// when the file actually changed (a restart drops every client for a moment).
pub async fn apply(m: &Mqtt) {
    let path = conf_path();
    let want = render(m);
    let have = std::fs::read_to_string(&path).ok();
    if want == have {
        return;
    }
    let res = match &want {
        Some(text) => {
            if let Some(dir) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, text)
        }
        None => std::fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }),
    };
    if let Err(e) = res {
        eprintln!("iod/mqtt: cannot write {path}: {e}");
        return;
    }
    eprintln!("iod/mqtt: bridge {} — restarting mosquitto", if want.is_some() { "configured" } else { "removed" });
    let _ = tokio::process::Command::new("/etc/init.d/S50mosquitto").arg("restart").status().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m() -> Mqtt {
        Mqtt {
            bridge: true,
            url: "mqtts://house.lan:8883".into(),
            username: "u".into(),
            password: "p".into(),
            prefix: "openhc".into(),
            client_id: "box".into(),
            discovery: "homeassistant".into(),
            ..Default::default()
        }
    }
    #[test]
    fn renders_a_two_way_bridge_with_tls() {
        let s = render(&m()).unwrap();
        assert!(s.contains("address house.lan:8883"));
        assert!(s.contains("topic openhc/box/# both 1"));
        assert!(s.contains("topic homeassistant/# out 1"));
        assert!(s.contains("remote_username u"));
        assert!(s.contains("bridge_capath /etc/ssl/certs"));
    }
    #[test]
    fn no_bridge_without_url_or_flag() {
        assert!(render(&Mqtt { url: "".into(), ..m() }).is_none());
        assert!(render(&Mqtt { bridge: false, ..m() }).is_none());
    }
    #[test]
    fn plain_url_defaults() {
        assert_eq!(split_url("mqtt://10.0.0.5"), ("10.0.0.5".into(), 1883, false));
        assert_eq!(split_url("ssl://h:1"), ("h".into(), 1, true));
    }
}
