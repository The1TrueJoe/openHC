//! openhc-control4: put a freshly installed openHC box back into a Control4
//! project — the on-screen navigator on its video output and its IO as
//! ordinary Control4 connections (https://github.com/The1TrueJoe/openhc-control4).
//!
//! Two artefacts come from that repository's newest release: the board's
//! Composer driver (`openhc-<model>.c4z`, saved where the user will find it), which every board
//! uses for its IO, and on the boards with an on-screen view the box side
//! (`openhc-control4.tar.gz`, unpacked into `/data/extensions`, which openHC's
//! `S96ohc-extensions` starts at every boot).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ohc_flash_core::Board;
use ohc_flash_transport as tp;

use crate::{updates, Event, Progress, Zigbee};

pub const REPO: &str = "The1TrueJoe/openhc-control4";
const TARBALL: &str = "openhc-control4.tar.gz";
const DIR: &str = "/data/extensions";

/// The board's Composer driver: one per model (`openhc-ea3.c4z`, `openhc-iox.c4z`).
pub fn driver_name(board: &Board) -> String {
    let model = board.name.split('-').next().unwrap_or(board.name);
    let model = if model.starts_with("iox") { "iox" } else { model };
    format!("openhc-{model}.c4z")
}

/// The boards the box side runs on: the x86_64 ones with openHC's on-screen
/// view (`wpe`) and a persistent `/data`. Every other board (HC-250, CA-1, IO
/// Extender) gets the driver alone, for its IO.
pub fn runs_on_box(board: &Board) -> bool {
    board.name.starts_with("ea") || board.name == "hc800"
}

/// What was downloaded: the box side if asked for, already gunzipped (busybox
/// tar on the box has no -z), and where the driver was saved.
pub struct Bundle {
    pub tar: Option<Vec<u8>>,
    pub driver: PathBuf,
}

/// The user's Downloads folder, or the temp dir when there is none.
pub fn driver_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// Download the newest release: the board's driver into `dir`, and with
/// `with_box` the box side into memory.
pub fn fetch(dir: &Path, board: &Board, with_box: bool, p: &Progress) -> Result<Bundle> {
    p.emit(Event::step("downloading the Control4 extension".into()));
    let rel = updates::newest_from(REPO)?;
    let url = |name: &str| {
        rel.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.url.clone())
            .with_context(|| format!("{REPO} {} has no {name}", rel.tag_name))
    };
    let tar = if with_box {
        let tmp = std::env::temp_dir().join(TARBALL);
        updates::download(&url(TARBALL)?, &tmp)?;
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(&tmp)?)
            .read_to_end(&mut tar)
            .context("unpacking the extension")?;
        let _ = std::fs::remove_file(&tmp);
        Some(tar)
    } else {
        None
    };
    let name = driver_name(board);
    let driver = dir.join(&name);
    updates::download(&url(&name)?, &driver)?;
    p.emit(Event::detail(format!("{REPO} {}: driver saved to {}", rel.tag_name, driver.display())));
    Ok(Bundle { tar, driver })
}

/// Unpack the box side, start it, point the on-screen view at the navigator,
/// and give the radio to the Zigbee asked for.
pub fn install(ssh: &tp::Ssh, b: &Bundle, zigbee: Zigbee, p: &Progress) -> Result<()> {
    let tar = b.tar.as_deref().context("the box side was not downloaded")?;
    p.emit(Event::step("setting the controller up for Control4".into()));
    ssh.put_stream(
        tar,
        &format!(
            "mkdir -p {DIR} && tar xf - -C {DIR} && {DIR}/control4/init start && {DIR}/control4/init display"
        ),
    )
    .map_err(|e| anyhow::anyhow!("installing the Control4 extension: {e}"))?;
    if zigbee == Zigbee::Control4 {
        p.emit(Event::step("giving the Zigbee radio to Control4 (Control4's own firmware)".into()));
        let out = ssh
            .run(&format!("{DIR}/control4/init zigbee control4 2>&1"), false)
            .map_err(|e| anyhow::anyhow!("switching the radio to Control4 Zigbee: {e}"))?;
        p.emit(Event::detail(out.trim().to_string()));
    }
    if !ssh.test("-x /etc/init.d/S96ohc-extensions") {
        p.emit(Event::warn(
            "this openHC image predates extensions: Control4 runs now, but update openHC so it \
             starts again after a reboot"
                .into(),
        ));
    }
    Ok(())
}

/// After an install reboots the box, find it again running openHC: wait for
/// `old_ip` to go quiet (so the box is not caught before it restarts), then
/// look at the old address and, by `mac`, wherever its new DHCP lease put it.
pub fn find_again(old_ip: &str, mac: Option<&str>, secs: u64, p: &Progress) -> Option<(String, tp::Ssh)> {
    p.emit(Event::step("waiting for openHC to come up".into()));
    let gone_by = Instant::now() + Duration::from_secs(120);
    while Instant::now() < gone_by && tp::ssh_port_open(old_ip) {
        std::thread::sleep(Duration::from_secs(2));
    }
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let mut ips = vec![old_ip.to_string()];
        if let Some(mac) = mac {
            ips.extend(
                tp::discover(false)
                    .into_iter()
                    .filter(|f| f.mac.as_deref().is_some_and(|m| same_mac(m, mac)))
                    .map(|f| f.ip),
            );
        }
        for ip in ips {
            if !tp::ssh_port_open(&ip) {
                continue;
            }
            if let Some(ssh) = tp::first_working_login(&ip) {
                if ssh.test("-f /etc/openhc-release") {
                    if ip != old_ip {
                        p.emit(Event::observed(format!("it came back at {ip}")));
                    }
                    return Some((ip, ssh));
                }
            }
        }
        std::thread::sleep(Duration::from_secs(10));
    }
    None
}

/// MACs compare as bytes: ARP on macOS prints `0:f:ff:…`, Linux `00:0f:ff:…`.
fn same_mac(a: &str, b: &str) -> bool {
    let bytes = |s: &str| s.split([':', '-']).map(|x| u8::from_str_radix(x, 16).ok()).collect::<Vec<_>>();
    let (a, b) = (bytes(a), bytes(b));
    a.len() == 6 && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_match_across_arp_spellings() {
        assert!(same_mac("0:f:ff:1f:35:65", "00:0F:FF:1F:35:65"));
        assert!(!same_mac("0:f:ff:1f:35:65", "00:0f:ff:1f:35:66"));
        assert!(!same_mac("garbage", "garbage"));
    }

    #[test]
    fn box_side_only_where_there_is_an_onscreen_view() {
        let b = |n| ohc_flash_core::board::by_name(n).unwrap();
        assert!(runs_on_box(b("ea3-v2")) && runs_on_box(b("hc800")));
        assert!(!runs_on_box(b("hc250")) && !runs_on_box(b("ca1")) && !runs_on_box(b("ioxv1")));
    }

    #[test]
    fn one_driver_per_model() {
        let d = |n| driver_name(ohc_flash_core::board::by_name(n).unwrap());
        assert_eq!(d("ea1-v2-poe"), "openhc-ea1.c4z");
        assert_eq!(d("ea5-v1"), "openhc-ea5.c4z");
        assert_eq!(d("ioxv1"), "openhc-iox.c4z");
        assert_eq!(d("hc800"), "openhc-hc800.c4z");
    }
}
