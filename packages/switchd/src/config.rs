//! The switch configuration document — what the operator wants the ports to be.
//!
//! This is the ONE piece of switch state that is not derivable from the running
//! system, so it is persisted as JSON (serde, per the owner's rule: config is
//! REST + a serde file, not a shell-sourced one) and re-applied on every boot.
//! Everything else switchd reports — carrier, speed, stats, which bridge a port
//! is in right now — is read back from the kernel, never stored here.
//!
//! The model mirrors how a BCM53125 is actually used through Linux DSA:
//!
//!   * `Managed`  — the default. Every port is a member of one bridge (`br-lan`),
//!                  so the box behaves as an ordinary unmanaged/managed L2 switch
//!                  and the b53 offloads the forwarding in hardware. One L3
//!                  address, on the bridge.
//!   * `Isolated` — no bridge. Each port is its own routed netdev with its own
//!                  address. This is the "two separate IP interfaces" case.
//!   * `Custom`   — the power mode: an explicit bridge membership plus 802.1Q
//!                  VLAN filtering (per-port PVID / tagged / untagged), with any
//!                  port left out of the bridge treated as routed like `Isolated`.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a layer-3 interface (the bridge, or a routed port) gets its address.
/// Internally tagged so the JSON reads `{"mode":"static","addr":"10.0.0.5/24"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Addressing {
    /// Lease over DHCP. switchd does not run the client itself — see the note on
    /// the uplink handoff in apply.rs — it names the interface for S40net.
    Dhcp,
    /// A fixed address (CIDR, e.g. `10.0.0.5/24`) and an optional default gateway.
    Static {
        addr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gw: Option<String>,
    },
    /// Up, but no IP — e.g. a port that only carries tagged VLANs.
    None,
}

impl Default for Addressing {
    fn default() -> Self {
        Addressing::Dhcp
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Managed,
    Isolated,
    Custom,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Managed
    }
}

fn default_true() -> bool {
    true
}

/// Per-port settings. Which fields matter depends on the mode: `enabled` always;
/// `addressing` only when the port is routed (Isolated, or a Custom port left out
/// of the bridge); the VLAN fields only when the bridge has `vlan_filtering`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PortConfig {
    /// Admin state. A disabled port is brought down (and dropped from the bridge)
    /// but stays in the config, so re-enabling it is one flag, not a re-add.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Addressing for a ROUTED port (Isolated mode, or a Custom port not in the
    /// bridge). Ignored for a bridged port — the address lives on the bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressing: Option<Addressing>,
    /// 802.1Q port VLAN id for untagged ingress (Custom + vlan_filtering).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvid: Option<u16>,
    /// VLAN ids this port carries tagged (Custom + vlan_filtering).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tagged: Vec<u16>,
    /// VLAN id this port carries untagged, if different from `pvid`. Usually
    /// equal to `pvid`; set both for the common access-port case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untagged: Option<u16>,
}

impl Default for PortConfig {
    fn default() -> Self {
        PortConfig {
            enabled: true,
            addressing: None,
            pvid: None,
            tagged: Vec::new(),
            untagged: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BridgeConfig {
    /// The bridge netdev name. `br-lan` by convention; the one address a managed
    /// switch presents lives here.
    #[serde(default = "default_bridge_name")]
    pub name: String,
    /// Spanning tree. ON by default: an openHC switch commonly has both jacks on
    /// the same upstream network, and STP is what keeps that from being an L2
    /// loop. The b53 offloads the port states.
    #[serde(default = "default_true")]
    pub stp: bool,
    /// 802.1Q VLAN filtering on the bridge. Off by default (a plain managed
    /// switch); turned on for Custom/VLAN setups, where per-port pvid/tagged/
    /// untagged then take effect.
    #[serde(default)]
    pub vlan_filtering: bool,
    /// Which ports are in the bridge. Defaults to every managed port.
    #[serde(default)]
    pub members: Vec<String>,
    /// How the bridge gets its L3 address.
    #[serde(default)]
    pub addressing: Addressing,
}

fn default_bridge_name() -> String {
    "br-lan".into()
}

impl Default for BridgeConfig {
    fn default() -> Self {
        BridgeConfig {
            name: default_bridge_name(),
            stp: true,
            vlan_filtering: false,
            members: Vec::new(),
            addressing: Addressing::Dhcp,
        }
    }
}

/// A VLAN definition (id + a friendly name for the UI). The membership lives on
/// the ports (pvid/tagged/untagged); this list is what the UI offers to assign.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Vlan {
    pub id: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SwitchConfig {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub bridge: BridgeConfig,
    /// Per-port overrides, keyed by DSA slave name (`lan1`, `lan2`). A port with
    /// no entry uses `PortConfig::default()` (enabled, no routed address).
    #[serde(default)]
    pub ports: BTreeMap<String, PortConfig>,
    #[serde(default)]
    pub vlans: Vec<Vlan>,
}

impl SwitchConfig {
    /// The default a box has before anyone configures it: a managed switch with
    /// every port bridged into `br-lan` and the bridge on DHCP. `ports` is the
    /// list this board actually has (from `OHC_DSA_PORTS`), so the bridge's
    /// member list and the per-port map are seeded from real hardware.
    pub fn default_for(ports: &[String]) -> SwitchConfig {
        SwitchConfig {
            mode: Mode::Managed,
            bridge: BridgeConfig {
                members: ports.to_vec(),
                ..BridgeConfig::default()
            },
            ports: ports
                .iter()
                .map(|p| (p.clone(), PortConfig::default()))
                .collect(),
            vlans: Vec::new(),
        }
    }

    /// `PortConfig` for a port, falling back to the default for an unlisted one.
    pub fn port(&self, name: &str) -> PortConfig {
        self.ports.get(name).cloned().unwrap_or_default()
    }

    /// Validate against the real port list and against itself. Returns a list of
    /// human-readable problems; empty means good. Applying an invalid config is
    /// refused so a typo cannot strand the box (e.g. a bridge member that is not
    /// a real port, which would leave the bridge empty and the box offline).
    pub fn validate(&self, ports: &[String]) -> Vec<String> {
        let mut errs = Vec::new();
        let known = |p: &String| ports.iter().any(|k| k == p);
        for m in &self.bridge.members {
            if !known(m) {
                errs.push(format!("bridge member '{m}' is not a switch port"));
            }
        }
        for p in self.ports.keys() {
            if !known(p) {
                errs.push(format!("port '{p}' is not a switch port"));
            }
        }
        if matches!(self.mode, Mode::Managed) && self.bridge.members.is_empty() {
            errs.push("managed mode with no bridge members would leave the box offline".into());
        }
        errs
    }
}

/// Where the config lives. Persistent on EA (the eMMC is the rootfs), so
/// `/etc/openhc/switch.json` survives reboots. `SWITCHD_CONFIG` overrides it.
pub fn config_path() -> String {
    std::env::var("SWITCHD_CONFIG").unwrap_or_else(|_| "/etc/openhc/switch.json".into())
}

/// Load the persisted config, or the sensible default for this board's ports if
/// there is none (or it is unreadable — a corrupt file must not strand the box;
/// the default is a reachable managed switch).
pub fn load(ports: &[String]) -> SwitchConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(t) => match serde_json::from_str::<SwitchConfig>(&t) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("switchd: {path} is not valid config ({e}); using the managed default");
                SwitchConfig::default_for(ports)
            }
        },
        Err(_) => SwitchConfig::default_for(ports),
    }
}

/// Persist the config (pretty JSON, so it is hand-editable too). Creates the
/// directory if needed.
pub fn save(cfg: &SwitchConfig) -> std::io::Result<()> {
    let path = config_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".into());
    std::fs::write(&path, body)
}

/// The managed ports, from `OHC_DSA_PORTS` in board.env. Empty means this board
/// has no managed switch and switchd has nothing to do.
pub fn dsa_ports() -> Vec<String> {
    // The daemon's init sources board.env and exports OHC_DSA_PORTS; honour the
    // same variable directly so `switchd` run by hand behaves identically.
    std::env::var("OHC_DSA_PORTS")
        .unwrap_or_default()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}
