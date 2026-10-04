//! Turn a [`SwitchConfig`] into kernel state, idempotently, with iproute2.
//!
//! WHY SHELL OUT to `ip`/`bridge` rather than talk netlink from Rust: iproute2
//! IS the proper tool here (the owner's "use proper tools, don't hand-roll"
//! rule), it is already in the image, and bridge-VLAN offload to the b53 is
//! exactly what `bridge vlan` was written to drive. A netlink crate would be a
//! second, thinner implementation of the same thing — and another dependency to
//! cross-compile for armv5. Each command is best-effort: a managed switch that
//! comes up with one wrong VLAN is better than one that refuses to come up, so
//! errors are collected and reported, not fatal.
//!
//! WHY the L3 handoff file: switchd owns layer 2 (bridge membership, VLANs,
//! admin state) and STATIC layer 3, but it must not run a second DHCP client
//! racing S40net on the same segment. So for a DHCP interface it writes
//! `/run/ohc/net.conf` naming the interface, and S40net leases it. One DHCP
//! client, one L2 owner, a one-line handoff. See board/common/.../S40net.
use crate::config::{Addressing, Mode, PortConfig, SwitchConfig};

/// Where switchd tells S40net which interface carries the box's L3 address.
pub const HANDOFF: &str = "/run/ohc/net.conf";

/// The outcome of an apply: what ran, and anything that failed.
#[derive(Debug, Default, serde::Serialize, utoipa::ToSchema)]
pub struct ApplyResult {
    pub ok: bool,
    /// The interface that now carries (or will lease) the box's address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l3_iface: Option<String>,
    pub steps: Vec<String>,
    pub errors: Vec<String>,
}

/// Run one iproute2 command. Returns Ok even on a non-zero exit only when
/// `tolerate` matches the stderr (e.g. "File exists" from re-creating a bridge),
/// so re-applying an already-correct config is quiet.
fn run(res: &mut ApplyResult, tolerate: &[&str], argv: &[&str]) {
    let shown = argv.join(" ");
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).output();
    match out {
        Ok(o) if o.status.success() => res.steps.push(shown),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            if tolerate.iter().any(|t| err.contains(t)) {
                res.steps.push(format!("{shown}  (already)"));
            } else {
                res.errors.push(format!("{shown}: {}", err.trim()));
            }
        }
        Err(e) => res.errors.push(format!("{shown}: {e}")),
    }
}

fn ip(res: &mut ApplyResult, tolerate: &[&str], args: &[&str]) {
    let mut argv = vec!["ip"];
    argv.extend_from_slice(args);
    run(res, tolerate, &argv);
}

fn bridge_cmd(res: &mut ApplyResult, tolerate: &[&str], args: &[&str]) {
    let mut argv = vec!["bridge"];
    argv.extend_from_slice(args);
    run(res, tolerate, &argv);
}

fn exists(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}")).exists()
}

/// Apply addressing to a routed L3 interface. DHCP is deferred to S40net (the
/// handoff); Static and None are applied here.
fn apply_l3(res: &mut ApplyResult, iface: &str, a: &Addressing) {
    match a {
        Addressing::Dhcp => {
            // Deferred — recorded as the handoff interface by the caller. Flush
            // any stale static so a dhcp-after-static switch does not keep both.
            ip(res, &[], &["addr", "flush", "dev", iface]);
            res.l3_iface = Some(iface.to_string());
        }
        Addressing::Static { addr, gw } => {
            ip(res, &[], &["addr", "flush", "dev", iface]);
            ip(res, &["File exists"], &["addr", "add", addr, "dev", iface]);
            if let Some(gw) = gw {
                ip(res, &["File exists", "RTNETLINK answers: File exists"],
                   &["route", "add", "default", "via", gw, "dev", iface]);
            }
            // A statically-addressed iface is still the box's L3 iface, but it is
            // already configured, so S40net must NOT lease it. We leave l3_iface
            // unset for S40net (handoff "static") but note it in steps.
            res.steps.push(format!("{iface}: static {addr}"));
        }
        Addressing::None => {
            ip(res, &[], &["addr", "flush", "dev", iface]);
        }
    }
}

fn set_up(res: &mut ApplyResult, iface: &str, up: bool) {
    ip(res, &[], &["link", "set", iface, if up { "up" } else { "down" }]);
}

/// Detach a port from any bridge it is currently in.
fn nomaster(res: &mut ApplyResult, port: &str) {
    if exists(port) {
        // `nomaster` errors harmlessly if it already has none on some versions.
        ip(res, &["is not a slave", "Invalid argument"], &["link", "set", port, "nomaster"]);
    }
}

/// Build the bridge and enslave its members. Returns the DHCP handoff decision
/// via `res.l3_iface`. Shared by Managed and Custom.
fn build_bridge(res: &mut ApplyResult, cfg: &SwitchConfig, ports: &[String]) {
    let br = &cfg.bridge.name;
    // Create the bridge if absent. "File exists" = already there.
    ip(res, &["File exists"], &["link", "add", "name", br, "type", "bridge"]);
    // STP and VLAN filtering are bridge attributes; set them each apply so a
    // config change takes effect without recreating the bridge.
    ip(res, &[], &["link", "set", br, "type", "bridge",
                   "stp_state", if cfg.bridge.stp { "1" } else { "0" }]);
    ip(res, &[], &["link", "set", br, "type", "bridge",
                   "vlan_filtering", if cfg.bridge.vlan_filtering { "1" } else { "0" }]);

    for p in ports {
        if !exists(p) {
            res.errors.push(format!("port {p} not present (DSA did not create it)"));
            continue;
        }
        let pc = cfg.port(p);
        if cfg.bridge.members.contains(p) && pc.enabled {
            ip(res, &["already a member", "File exists"], &["link", "set", p, "master", br]);
            set_up(res, p, true);
        } else {
            // Not a member (or disabled): out of the bridge, routed or down.
            nomaster(res, p);
            apply_routed_port(res, &pc, p);
        }
    }
    set_up(res, br, true);

    if cfg.bridge.vlan_filtering {
        program_vlans(res, cfg);
    }

    apply_l3(res, br, &cfg.bridge.addressing);
}

/// A port that is NOT in the bridge: its own address (or down if disabled).
fn apply_routed_port(res: &mut ApplyResult, pc: &PortConfig, port: &str) {
    if !pc.enabled {
        ip(res, &[], &["addr", "flush", "dev", port]);
        set_up(res, port, false);
        return;
    }
    set_up(res, port, true);
    match &pc.addressing {
        Some(a) => apply_l3(res, port, a),
        None => {} // routed but unconfigured — leave as-is (up, no address)
    }
}

/// Program the per-port 802.1Q VLAN table on a vlan_filtering bridge. The b53
/// offloads this. Best-effort and additive: we set each member port's pvid +
/// untagged + tagged vids. (Pruning stale vids is a follow-up — see the module
/// note; a superset is safe, an operator removing a VLAN re-applies cleanly
/// after the bridge is torn down and rebuilt on a mode change.)
fn program_vlans(res: &mut ApplyResult, cfg: &SwitchConfig) {
    for p in &cfg.bridge.members {
        let pc = cfg.port(p);
        if let Some(pvid) = pc.pvid {
            // pvid + untagged on the access vlan.
            let vid = pc.untagged.unwrap_or(pvid).to_string();
            let pvid_s = pvid.to_string();
            bridge_cmd(res, &["already configured", "RTNETLINK answers: File exists"],
                &["vlan", "add", "dev", p, "vid", &pvid_s, "pvid", "untagged"]);
            if vid != pvid_s {
                bridge_cmd(res, &["already configured", "RTNETLINK answers: File exists"],
                    &["vlan", "add", "dev", p, "vid", &vid, "untagged"]);
            }
        }
        for t in &pc.tagged {
            bridge_cmd(res, &["already configured", "RTNETLINK answers: File exists"],
                &["vlan", "add", "dev", p, "vid", &t.to_string()]);
        }
    }
}

/// Reconcile the running network to `cfg`. The CPU-side master (`eth0`) is left
/// to S39ea-switch/S40net; switchd only touches the DSA slaves and the bridge.
pub fn apply(cfg: &SwitchConfig, ports: &[String]) -> ApplyResult {
    let mut res = ApplyResult::default();

    match cfg.mode {
        Mode::Managed | Mode::Custom => build_bridge(&mut res, cfg, ports),
        Mode::Isolated => {
            // No bridge. Tear a stale one down so a mode switch is clean, then
            // each port stands alone with its own address.
            if exists(&cfg.bridge.name) {
                for p in ports {
                    nomaster(&mut res, p);
                }
                ip(&mut res, &["Cannot find device"], &["link", "del", &cfg.bridge.name]);
            }
            for p in ports {
                if !exists(p) {
                    res.errors.push(format!("port {p} not present (DSA did not create it)"));
                    continue;
                }
                apply_routed_port(&mut res, &cfg.port(p), p);
            }
        }
    }

    res.ok = res.errors.is_empty();
    write_handoff(&res, cfg);
    res
}

/// Tell S40net where the L3 address is and how to get it. One shell-sourced line
/// each, so S40net stays a tiny `. /run/ohc/net.conf`.
///   OHC_NET_L3_IFACE  the interface carrying the box address
///   OHC_NET_L3_MODE   dhcp | static | none  (dhcp = S40net should lease it)
fn write_handoff(res: &ApplyResult, cfg: &SwitchConfig) {
    let (iface, mode) = match cfg.mode {
        Mode::Managed | Mode::Custom => {
            let m = match &cfg.bridge.addressing {
                Addressing::Dhcp => "dhcp",
                Addressing::Static { .. } => "static",
                Addressing::None => "none",
            };
            (cfg.bridge.name.clone(), m)
        }
        Mode::Isolated => {
            // No single uplink. Name the first DHCP port if there is one, so the
            // box still leases something; otherwise the first configured port.
            let dhcp_port = cfg.ports.iter().find(|(_, pc)| {
                pc.enabled && matches!(pc.addressing, Some(Addressing::Dhcp))
            });
            match dhcp_port {
                Some((p, _)) => (p.clone(), "dhcp"),
                None => (res.l3_iface.clone().unwrap_or_default(), "static"),
            }
        }
    };
    if let Some(dir) = std::path::Path::new(HANDOFF).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = format!("OHC_NET_L3_IFACE={iface}\nOHC_NET_L3_MODE={mode}\n");
    let _ = std::fs::write(HANDOFF, body);
}
