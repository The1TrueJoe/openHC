//! WS-Discovery host: how Windows' File Explorer finds the box under Network.
//!
//! macOS finds SMB servers over mDNS (smb.rs publishes `_smb._tcp`); Windows
//! dropped NetBIOS browsing and uses WS-Discovery instead. There is no Rust
//! crate for it and Buildroot packages no responder (wsdd2's upstream is gone;
//! wsdd is Python), so this is the host half of the protocol, modelled
//! message for message on christgau/wsdd's `WSDHost`:
//!
//! * UDP 239.255.255.250:3702 — announce with Hello on start and Bye on stop;
//!   answer a Probe for `wsdp:Device` with a ProbeMatch, and a Resolve of our
//!   endpoint with a ResolveMatch carrying our metadata URL;
//! * HTTP :5357 `POST /<uuid>` — answer the metadata Get with this box's
//!   identity, including `pub:Computer` = `HOSTNAME/Workgroup:WORKGROUP`,
//!   which is the name Explorer shows and opens (\\HOSTNAME).
//!
//! Explorer then opens `\\HOSTNAME` — a single-label name, which Windows
//! resolves with LLMNR (avahi answers only mDNS `.local` names). So, as wsdd2
//! did, this also answers LLMNR A queries for the hostname (`llmnr`).
//!
//! The endpoint id is a UUIDv5 of the hostname, so it is stable across
//! restarts (as wsdd's default is). IPv4 only, as the boards are.
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const UDP_PORT: u16 = 3702;
pub const HTTP_PORT: u16 = 5357;

const WSA: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing";
const WSD: &str = "http://schemas.xmlsoap.org/ws/2005/04/discovery";
const WSDP: &str = "http://schemas.xmlsoap.org/ws/2006/02/devprof";
const WSA_ANON: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous";
const WSA_DISCOVERY: &str = "urn:schemas-xmlsoap-org:ws:2005:04:discovery";
const GET: &str = "http://schemas.xmlsoap.org/ws/2004/09/transfer/Get";
const GET_RESPONSE: &str = "http://schemas.xmlsoap.org/ws/2004/09/transfer/GetResponse";
const TYPES: &str = "wsdp:Device pub:Computer";

const NAMESPACES: &str = concat!(
    r#"xmlns:soap="http://www.w3.org/2003/05/soap-envelope" "#,
    r#"xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" "#,
    r#"xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" "#,
    r#"xmlns:wsx="http://schemas.xmlsoap.org/ws/2004/09/mex" "#,
    r#"xmlns:wsdp="http://schemas.xmlsoap.org/ws/2006/02/devprof" "#,
    r#"xmlns:pnpx="http://schemas.microsoft.com/windows/pnpx/2005/10" "#,
    r#"xmlns:pub="http://schemas.microsoft.com/windows/pub/2005/07""#
);

/// This box as a WSD host.
pub struct Host {
    pub hostname: String,
    pub workgroup: String,
    /// Stable endpoint id (UUIDv5 of the hostname).
    pub uuid: uuid::Uuid,
    instance_id: u64,
    // 32-bit: the ARMv5 boards have no 64-bit atomics (the workspace builds for all).
    message_number: AtomicU32,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Host {
    pub fn new(hostname: &str, workgroup: &str) -> Host {
        let instance_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs());
        Host {
            hostname: hostname.to_string(),
            workgroup: workgroup.to_string(),
            uuid: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, hostname.as_bytes()),
            instance_id,
            message_number: AtomicU32::new(0),
        }
    }

    fn urn(&self) -> String {
        self.uuid.urn().to_string()
    }

    fn xaddr(&self, ip: Ipv4Addr) -> String {
        format!("<wsd:XAddrs>http://{ip}:{HTTP_PORT}/{}</wsd:XAddrs>", self.uuid)
    }

    fn epr(&self) -> String {
        format!("<wsa:EndpointReference><wsa:Address>{}</wsa:Address></wsa:EndpointReference>", self.urn())
    }

    /// A whole SOAP message. UDP messages carry an AppSequence; `relates_to`
    /// is the MessageID being answered.
    fn message(&self, to: &str, action: &str, relates_to: Option<&str>, udp: bool, body: &str) -> String {
        let mut h = format!(
            "<wsa:To>{}</wsa:To><wsa:Action>{}</wsa:Action><wsa:MessageID>{}</wsa:MessageID>",
            esc(to),
            esc(action),
            uuid::Uuid::new_v4().urn()
        );
        if let Some(r) = relates_to {
            h += &format!("<wsa:RelatesTo>{}</wsa:RelatesTo>", esc(r));
        }
        if udp {
            let n = self.message_number.fetch_add(1, Ordering::Relaxed);
            h += &format!(
                r#"<wsd:AppSequence InstanceId="{}" SequenceId="{}" MessageNumber="{n}"/>"#,
                self.instance_id,
                uuid::Uuid::new_v4().urn()
            );
        }
        format!(r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope {NAMESPACES}><soap:Header>{h}</soap:Header><soap:Body>{body}</soap:Body></soap:Envelope>"#)
    }

    pub fn hello(&self, ip: Ipv4Addr) -> String {
        let body = format!("<wsd:Hello>{}{}<wsd:MetadataVersion>1</wsd:MetadataVersion></wsd:Hello>", self.epr(), self.xaddr(ip));
        self.message(WSA_DISCOVERY, &format!("{WSD}/Hello"), None, true, &body)
    }

    pub fn bye(&self) -> String {
        let body = format!("<wsd:Bye>{}</wsd:Bye>", self.epr());
        self.message(WSA_DISCOVERY, &format!("{WSD}/Bye"), None, true, &body)
    }

    /// The reply to one UDP datagram, if it is a Probe or Resolve for us.
    pub fn answer(&self, datagram: &str, ip: Ipv4Addr) -> Option<String> {
        let doc = roxmltree::Document::parse(datagram).ok()?;
        let find = |ns: &str, name: &str| doc.descendants().find(|n| n.has_tag_name((ns, name)));
        let msg_id = find(WSA, "MessageID")?.text()?.trim().to_string();
        let action = find(WSA, "Action")?.text()?.trim().to_string();
        if action == format!("{WSD}/Probe") {
            // Like wsdd: only a plain device probe, without scopes.
            if find(WSD, "Scopes").is_some_and(|s| s.text().is_some_and(|t| !t.trim().is_empty())) {
                return None;
            }
            let types = find(WSD, "Types")?.text()?.trim().to_string();
            if types != "wsdp:Device" {
                return None;
            }
            let body = format!(
                "<wsd:ProbeMatches><wsd:ProbeMatch>{}<wsd:Types>{TYPES}</wsd:Types><wsd:MetadataVersion>1</wsd:MetadataVersion></wsd:ProbeMatch></wsd:ProbeMatches>",
                self.epr()
            );
            return Some(self.message(WSA_ANON, &format!("{WSD}/ProbeMatches"), Some(&msg_id), true, &body));
        }
        if action == format!("{WSD}/Resolve") {
            let addr = doc
                .descendants()
                .find(|n| n.has_tag_name((WSA, "EndpointReference")))?
                .descendants()
                .find(|n| n.has_tag_name((WSA, "Address")))?
                .text()?
                .trim()
                .to_string();
            if addr != self.urn() {
                return None;
            }
            let body = format!(
                "<wsd:ResolveMatches><wsd:ResolveMatch>{}<wsd:Types>{TYPES}</wsd:Types>{}<wsd:MetadataVersion>1</wsd:MetadataVersion></wsd:ResolveMatch></wsd:ResolveMatches>",
                self.epr(),
                self.xaddr(ip)
            );
            return Some(self.message(WSA_ANON, &format!("{WSD}/ResolveMatches"), Some(&msg_id), true, &body));
        }
        None
    }

    /// The reply to the metadata Get (HTTP), if the request is one.
    pub fn metadata(&self, request: &str) -> Option<String> {
        let doc = roxmltree::Document::parse(request).ok()?;
        let find = |ns: &str, name: &str| doc.descendants().find(|n| n.has_tag_name((ns, name)));
        let action = find(WSA, "Action")?.text()?.trim().to_string();
        if action != GET {
            return None;
        }
        let msg_id = find(WSA, "MessageID")?.text()?.trim().to_string();
        let body = format!(
            concat!(
                r#"<wsx:Metadata>"#,
                r#"<wsx:MetadataSection Dialect="{wsdp}/ThisDevice"><wsdp:ThisDevice><wsdp:FriendlyName>openHC {host}</wsdp:FriendlyName><wsdp:FirmwareVersion>1.0</wsdp:FirmwareVersion><wsdp:SerialNumber>1</wsdp:SerialNumber></wsdp:ThisDevice></wsx:MetadataSection>"#,
                r#"<wsx:MetadataSection Dialect="{wsdp}/ThisModel"><wsdp:ThisModel><wsdp:Manufacturer>openHC</wsdp:Manufacturer><wsdp:ModelName>openHC</wsdp:ModelName><pnpx:DeviceCategory>Computers</pnpx:DeviceCategory></wsdp:ThisModel></wsx:MetadataSection>"#,
                r#"<wsx:MetadataSection Dialect="{wsdp}/Relationship"><wsdp:Relationship Type="{wsdp}/host"><wsdp:Host>{epr}<wsdp:Types>pub:Computer</wsdp:Types><wsdp:ServiceId>{urn}</wsdp:ServiceId><pub:Computer>{computer}</pub:Computer></wsdp:Host></wsdp:Relationship></wsx:MetadataSection>"#,
                r#"</wsx:Metadata>"#
            ),
            wsdp = WSDP,
            host = esc(&self.hostname),
            epr = self.epr(),
            urn = self.urn(),
            computer = esc(&format!("{}/Workgroup:{}", self.hostname.to_uppercase(), self.workgroup.to_uppercase())),
        );
        Some(self.message(WSA_ANON, GET_RESPONSE, Some(&msg_id), false, &body))
    }
}

/// The local IPv4 address used to reach `peer` (which interface answers).
fn local_ip_for(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((peer, UDP_PORT)).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_unspecified() => Some(*a.ip()),
        _ => None,
    }
}

/// Serve WS-Discovery on UDP until the task is dropped. `enabled` is checked
/// per datagram, so turning sharing off makes the box stop answering.
pub async fn run(host: Arc<Host>, enabled: impl Fn() -> bool + Send + 'static) {
    let sock = loop {
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, UDP_PORT)).await {
            Ok(s) => break s,
            Err(e) => {
                eprintln!("storaged: wsd: cannot bind udp/{UDP_PORT}: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    };
    if let Err(e) = sock.join_multicast_v4(GROUP, Ipv4Addr::UNSPECIFIED) {
        eprintln!("storaged: wsd: cannot join {GROUP}: {e}");
    }
    let _ = sock.set_multicast_loop_v4(false);
    let _ = sock.set_multicast_ttl_v4(1);

    // Hello (repeated, as UDP multicast may drop one).
    if enabled() {
        if let Some(ip) = local_ip_for(GROUP) {
            let hello = host.hello(ip);
            for _ in 0..3 {
                let _ = sock.send_to(hello.as_bytes(), (GROUP, UDP_PORT)).await;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }

    let mut seen: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut buf = vec![0u8; 32768];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
        let SocketAddr::V4(from) = from else { continue };
        if !enabled() {
            continue;
        }
        let text = String::from_utf8_lossy(&buf[..n]);
        // Multicast arrives more than once; answer each MessageID once.
        let id = roxmltree::Document::parse(&text)
            .ok()
            .and_then(|d| d.descendants().find(|n| n.has_tag_name((WSA, "MessageID"))).and_then(|n| n.text().map(str::to_string)));
        let Some(id) = id else { continue };
        if seen.contains(&id) {
            continue;
        }
        seen.push_back(id);
        if seen.len() > 16 {
            seen.pop_front();
        }
        let Some(ip) = local_ip_for(*from.ip()) else { continue };
        if let Some(reply) = host.answer(&text, ip) {
            // A short random delay, as the spec asks of answers to multicast.
            tokio::time::sleep(Duration::from_millis(u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 200)).await;
            let _ = sock.send_to(reply.as_bytes(), SocketAddr::V4(from)).await;
        }
    }
}

const LLMNR_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 252);
const LLMNR_PORT: u16 = 5355;

/// The answer to one LLMNR query (RFC 4795, DNS wire format), if it asks for
/// the A record of `hostname` (case-insensitively). `ip` is the address to
/// give. Anything else — another name, AAAA, a malformed packet — gets no
/// answer, which is what a non-authoritative LLMNR responder sends.
pub fn llmnr_answer(q: &[u8], hostname: &str, ip: Ipv4Addr) -> Option<Vec<u8>> {
    if q.len() < 12 {
        return None;
    }
    let flags = u16::from_be_bytes([q[2], q[3]]);
    let qdcount = u16::from_be_bytes([q[4], q[5]]);
    // A query (QR=0), standard opcode, exactly one question.
    if flags & 0x8000 != 0 || (flags >> 11) & 0xf != 0 || qdcount != 1 {
        return None;
    }
    let mut i = 12;
    let mut labels: Vec<String> = Vec::new();
    loop {
        let len = *q.get(i)? as usize;
        i += 1;
        if len == 0 {
            break;
        }
        if len > 63 {
            return None; // no compression in a question
        }
        labels.push(String::from_utf8_lossy(q.get(i..i + len)?).into_owned());
        i += len;
    }
    let qtype = u16::from_be_bytes([*q.get(i)?, *q.get(i + 1)?]);
    let qclass = u16::from_be_bytes([*q.get(i + 2)?, *q.get(i + 3)?]);
    let question_end = i + 4;
    if labels.len() != 1 || !labels[0].eq_ignore_ascii_case(hostname) || qclass != 1 || (qtype != 1 && qtype != 255) {
        return None;
    }
    let mut r = Vec::with_capacity(question_end + 16);
    r.extend_from_slice(&q[0..2]); // ID
    r.extend_from_slice(&0x8000u16.to_be_bytes()); // QR=1, no conflict
    r.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    r.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT
    r.extend_from_slice(&[0, 0, 0, 0]); // NSCOUNT, ARCOUNT
    r.extend_from_slice(&q[12..question_end]);
    r.extend_from_slice(&[0xc0, 0x0c]); // name: pointer to the question
    r.extend_from_slice(&1u16.to_be_bytes()); // A
    r.extend_from_slice(&1u16.to_be_bytes()); // IN
    r.extend_from_slice(&30u32.to_be_bytes()); // TTL (RFC 4795 suggests 30 s)
    r.extend_from_slice(&4u16.to_be_bytes());
    r.extend_from_slice(&ip.octets());
    Some(r)
}

/// Answer LLMNR queries for the hostname until the task is dropped.
pub async fn llmnr(hostname: String, enabled: impl Fn() -> bool + Send + 'static) {
    let sock = loop {
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, LLMNR_PORT)).await {
            Ok(s) => break s,
            Err(e) => {
                eprintln!("storaged: llmnr: cannot bind udp/{LLMNR_PORT}: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    };
    if let Err(e) = sock.join_multicast_v4(LLMNR_GROUP, Ipv4Addr::UNSPECIFIED) {
        eprintln!("storaged: llmnr: cannot join {LLMNR_GROUP}: {e}");
    }
    let mut buf = [0u8; 512];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
        let SocketAddr::V4(from4) = from else { continue };
        if !enabled() {
            continue;
        }
        let Some(ip) = local_ip_for(*from4.ip()) else { continue };
        if let Some(reply) = llmnr_answer(&buf[..n], &hostname, ip) {
            let _ = sock.send_to(&reply, from).await;
        }
    }
}

/// Withdraw (Bye), best effort, on shutdown.
pub fn bye(host: &Host) {
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        let _ = s.set_multicast_ttl_v4(1);
        let msg = host.bye();
        for _ in 0..2 {
            let _ = s.send_to(msg.as_bytes(), (GROUP, UDP_PORT));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> Host {
        Host::new("openhc-hc800-000FFF57B978", "WORKGROUP")
    }
    const IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 112);

    /// A Probe as Windows sends it (trimmed).
    const PROBE: &str = r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:wsdp="http://schemas.xmlsoap.org/ws/2006/02/devprof"><soap:Header><wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action><wsa:MessageID>urn:uuid:11111111-2222-3333-4444-555555555555</wsa:MessageID></soap:Header><soap:Body><wsd:Probe><wsd:Types>wsdp:Device</wsd:Types></wsd:Probe></soap:Body></soap:Envelope>"#;

    #[test]
    fn answers_a_device_probe() {
        let h = host();
        let r = h.answer(PROBE, IP).unwrap();
        let d = roxmltree::Document::parse(&r).unwrap();
        let text = |ns: &str, n: &str| d.descendants().find(|x| x.has_tag_name((ns, n))).and_then(|x| x.text()).unwrap().to_string();
        assert_eq!(text(WSA, "Action"), format!("{WSD}/ProbeMatches"));
        assert_eq!(text(WSA, "RelatesTo"), "urn:uuid:11111111-2222-3333-4444-555555555555");
        assert_eq!(text(WSA, "To"), WSA_ANON);
        assert_eq!(text(WSD, "Types"), "wsdp:Device pub:Computer");
        assert_eq!(text(WSA, "Address"), h.urn());
        assert!(d.descendants().any(|x| x.has_tag_name((WSD, "AppSequence"))));
    }

    #[test]
    fn ignores_other_probes() {
        let h = host();
        assert!(h.answer(&PROBE.replace(">wsdp:Device<", ">wscn:ScanDeviceType<"), IP).is_none());
        assert!(h.answer("not xml", IP).is_none());
    }

    #[test]
    fn resolves_only_our_endpoint() {
        let h = host();
        let resolve = |urn: &str| {
            PROBE
                .replace("discovery/Probe<", "discovery/Resolve<")
                .replace("<wsd:Probe><wsd:Types>wsdp:Device</wsd:Types></wsd:Probe>", &format!("<wsd:Resolve><wsa:EndpointReference><wsa:Address>{urn}</wsa:Address></wsa:EndpointReference></wsd:Resolve>"))
        };
        let r = h.answer(&resolve(&h.urn()), IP).unwrap();
        assert!(r.contains(&format!("<wsd:XAddrs>http://10.0.0.112:5357/{}</wsd:XAddrs>", h.uuid)), "{r}");
        assert!(r.contains("/ResolveMatches</wsa:Action>"));
        assert!(h.answer(&resolve("urn:uuid:00000000-0000-0000-0000-000000000000"), IP).is_none());
    }

    #[test]
    fn metadata_names_the_computer() {
        let h = host();
        let get = PROBE.replace("http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe", GET).replace("<wsd:Probe><wsd:Types>wsdp:Device</wsd:Types></wsd:Probe>", "");
        let r = h.metadata(&get).unwrap();
        roxmltree::Document::parse(&r).unwrap();
        assert!(r.contains("<pub:Computer>OPENHC-HC800-000FFF57B978/Workgroup:WORKGROUP</pub:Computer>"), "{r}");
        assert!(r.contains(&format!("<wsa:Action>{GET_RESPONSE}</wsa:Action>")));
        assert!(!r.contains("AppSequence"));
        assert!(h.metadata(PROBE).is_none());
    }

    /// An LLMNR query for `name`, type `qtype`.
    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        q.push(name.len() as u8);
        q.extend_from_slice(name.as_bytes());
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&1u16.to_be_bytes());
        q
    }

    #[test]
    fn llmnr_answers_our_name_only() {
        let r = llmnr_answer(&query("OPENHC-HC800-000FFF57B978", 1), "openhc-hc800-000FFF57B978", IP).unwrap();
        assert_eq!(&r[0..2], &[0x12, 0x34]);
        assert_eq!(u16::from_be_bytes([r[2], r[3]]) & 0x8000, 0x8000);
        assert_eq!(u16::from_be_bytes([r[6], r[7]]), 1);
        assert_eq!(&r[r.len() - 4..], &[10, 0, 0, 112]);
        assert!(llmnr_answer(&query("other", 1), "openhc-hc800-000FFF57B978", IP).is_none());
        assert!(llmnr_answer(&query("openhc-hc800-000FFF57B978", 28), "openhc-hc800-000FFF57B978", IP).is_none());
        assert!(llmnr_answer(&[0u8; 5], "x", IP).is_none());
        let mut resp = query("x", 1);
        resp[2] = 0x80; // a response, not a query
        assert!(llmnr_answer(&resp, "x", IP).is_none());
    }

    #[test]
    fn endpoint_id_is_stable() {
        assert_eq!(host().uuid, host().uuid);
        let hello = host().hello(IP);
        roxmltree::Document::parse(&hello).unwrap();
        roxmltree::Document::parse(&host().bye()).unwrap();
    }
}
