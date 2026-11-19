//! EA return to stock over SSH, with no button, on any openHC image.
//!
//! The work happens on the box, in a boot-init run from RAM where p1 is free:
//! its `restore reimage-finalize` puts Control4's kernel back in its eMMC slot,
//! clears openHC's autoscript and reimages p1 from p2 (packages/restore). The
//! flasher makes that possible whatever the box runs:
//!
//! 1. Control4's Zigbee firmware goes back on the radio (radio.rs).
//! 2. It stages a current boot-init (and the real kernel, if the box has none)
//!    from the board's image bundle, and a one-shot `S00arestore` that kexecs
//!    them at the very start of the next boot. Older images have neither.
//! 3. It drops the restore flag and reboots. A gap boot-init that knows the flag
//!    restores in place, before p1 is even mounted; any other boots p1 far
//!    enough for `S00arestore` to hand off.
//! 4. It waits for the box to answer as stock Control4 (SDDP, from its MAC).

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use ohc_flash_core::board::Family;
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};

const STAGE: &str = "/boot/ohc-restore";
const FLAG: &str = "/.ohc-restore-stock";
const HANDOFF_PATH: &str = "/etc/init.d/S00arestore";
/// How long Control4 may take to come up: the p1 unpack plus its first boot.
const STOCK_WAIT: Duration = Duration::from_secs(20 * 60);

/// Runs before S00kexec, on the bootstrap kernel (the GPU never used), so the
/// kexec is the same early hand-off as a normal boot.
const HANDOFF: &str = r#"#!/bin/sh
# One-shot return to stock (ohc-flash restore): boot the real kernel with the
# staged boot-init as its RAM disk; it restores before it would pivot to p1.
[ "$1" = start ] || exit 0
rm -f /etc/init.d/S00arestore; sync
mount -t proc proc /proc 2>/dev/null
case " $(cat /proc/cmdline) " in *" ohc_kexec="*) exit 0 ;; esac
D=/boot/ohc-restore
K=$D/bzImage; [ -f "$K" ] || K=/boot/bzImage
A=; for a in $(cat /proc/cmdline); do case "$a" in ohc.*) A="$A $a" ;; esac; done
echo "S00arestore: return to stock: handing off to $K with $D/boot-init.cpio.gz"
if kexec -l "$K" --initrd="$D/boot-init.cpio.gz" \
	--command-line="console=uart8250,io,0x3f8 pci=realloc,nocrs,routeirq intel_iommu=off root=/dev/mmcblk0p1 rootwait rw ohc_kexec=1$A"; then
	sync
	kexec -e
fi
rm -f /.ohc-restore-stock; sync
echo "S00arestore: hand-off failed; booting openHC" >&2
exit 0
"#;

pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    restore_with(ssh, None, p)
}

/// `images`: a release directory or zip to take the boot-init and kernel from,
/// instead of the board's bundle on the latest GitHub release.
pub fn restore_with(ssh: &Ssh, images: Option<&Path>, p: &Progress) -> Result<()> {
    if crate::restore_bin(ssh).is_none() && ssh.read_file("/opt/ohc/board.env").is_none() {
        bail!("this box is running stock Control4; there is nothing of openHC's boot to remove from here");
    }
    let id = ohc_flash_transport::identify(ssh);
    let board = id.board.filter(|b| b.family == Family::Ea).ok_or_else(|| {
        anyhow!("not an EA this flasher can identify ({}); refusing to restore", id.describe())
    })?;
    let mac = ssh.read_file("/sys/class/net/eth0/address").map(|m| m.trim().to_ascii_lowercase());

    crate::radio::restore(ssh, p)?;

    p.emit(Event::step(format!("fetching the {} restore boot-init", board.name)));
    let bootinit = fetch(images, board.name, "boot-init.cpio.gz")?;
    if !knows_flag(&bootinit) {
        bail!("that boot-init predates the no-button restore; use a newer release (--images)");
    }
    let kernel = if ssh.test("-s /boot/bzImage") {
        None
    } else {
        p.emit(Event::detail("this image has no real kernel on p1; fetching one".into()));
        Some(fetch(images, board.name, "bzImage-real")?)
    };

    p.emit(Event::step("staging the RAM restore on p1".into()));
    ssh.run(&format!("mkdir -p {STAGE}"), true).map_err(|e| anyhow!("{e}"))?;
    put(ssh, &bootinit, &format!("{STAGE}/boot-init.cpio.gz"))?;
    if let Some(k) = &kernel {
        put(ssh, k, &format!("{STAGE}/bzImage"))?;
    }
    put(ssh, HANDOFF.as_bytes(), HANDOFF_PATH)?;
    ssh.run(&format!("chmod 755 {HANDOFF_PATH} && : > {FLAG} && sync"), true)
        .map_err(|e| anyhow!("{e}"))?;

    p.emit(Event::step(
        "rebooting: from RAM the box puts Control4's kernel back, clears the autoscript and \
         reimages p1, then boots stock"
            .into(),
    ));
    let _ = ssh.run("sync; reboot", false);

    wait_for_stock(mac.as_deref(), p)
}

fn fetch(images: Option<&Path>, board: &str, name: &str) -> Result<Vec<u8>> {
    match images {
        Some(dir) if dir.is_dir() => Ok(std::fs::read(dir.join(name))?),
        Some(zip) => {
            let mut z = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
            crate::zipfetch::member(&mut z, name)
        }
        None => {
            let rel = crate::updates::latest_release().or_else(|_| crate::updates::dev_release())?;
            let asset = rel
                .image_asset(board)
                .ok_or_else(|| anyhow!("release {} has no bundle for {board}", rel.tag_name))?;
            crate::zipfetch::fetch_member(&asset.url, name)
        }
    }
}

/// Does this boot-init's /init act on the restore flag?
fn knows_flag(cpio_gz: &[u8]) -> bool {
    use std::io::Read;
    let mut raw = Vec::new();
    if flate2::read::GzDecoder::new(cpio_gz).read_to_end(&mut raw).is_err() {
        return false;
    }
    let flag = FLAG.trim_start_matches('/').as_bytes();
    raw.windows(flag.len()).any(|w| w == flag)
}

/// Write `data` to `path` on the box and check its size there.
fn put(ssh: &Ssh, data: &[u8], path: &str) -> Result<()> {
    ssh.put_stream(data, &format!("cat > {path}")).map_err(|e| anyhow!("{e}"))?;
    let got = ssh.run(&format!("wc -c < {path}"), true).map_err(|e| anyhow!("{e}"))?;
    if got.trim().parse::<usize>().ok() != Some(data.len()) {
        bail!("{path} arrived as {} bytes, not {}", got.trim(), data.len());
    }
    Ok(())
}

/// Wait for a Control4 controller with this MAC to answer SDDP. openHC never
/// answers SDDP, so that answer means stock is up.
fn wait_for_stock(mac: Option<&str>, p: &Progress) -> Result<()> {
    let Some(mac) = mac else {
        p.emit(Event::warn("could not read the box's MAC; check by hand that it boots stock".into()));
        return Ok(());
    };
    p.emit(Event::step(format!("waiting for {mac} to come back as stock Control4 (up to 20 min)")));
    let start = Instant::now();
    while start.elapsed() < STOCK_WAIT {
        std::thread::sleep(Duration::from_secs(30));
        let found = ohc_flash_transport::discover(true);
        if let Some(f) = found.iter().find(|f| f.is_director && f.mac.as_deref().is_some_and(|m| same_mac(m, mac))) {
            p.emit(Event::resolved(format!(
                "stock Control4 is up at {} ({}), after {} min",
                f.ip,
                f.model.as_deref().unwrap_or("controller"),
                start.elapsed().as_secs() / 60
            )));
            return Ok(());
        }
    }
    bail!(
        "{mac} did not answer as stock Control4 within 20 min. If it is not on the network, hold \
         the recessed factory-restore button: Control4's recovery reimages p1 and boots stock."
    )
}

/// ARP tables drop leading zeros (`0:f:ff:…`); compare octet by octet.
fn same_mac(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> Vec<u8> { s.split(':').filter_map(|o| u8::from_str_radix(o, 16).ok()).collect() };
    let (x, y) = (norm(a), norm(b));
    x.len() == 6 && x == y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_compare_without_leading_zeros() {
        assert!(same_mac("0:f:ff:94:ee:2", "00:0f:ff:94:ee:02"));
        assert!(!same_mac("0:f:ff:94:ee:3", "00:0f:ff:94:ee:02"));
        assert!(!same_mac("", ""));
    }

    #[test]
    fn boot_init_flag_check() {
        use std::io::Write;
        let gz = |b: &[u8]| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(b).unwrap();
            e.finish().unwrap()
        };
        assert!(knows_flag(&gz(b"init: if [ -f \"$NEW/.ohc-restore-stock\" ]")));
        assert!(!knows_flag(&gz(b"init: if [ -f \"$NEW/.ohc-stock-pending\" ]")));
        assert!(!knows_flag(b"not gzip"));
    }

    #[test]
    fn handoff_runs_before_s00kexec_and_cleans_up() {
        assert!("S00arestore" < "S00kexec");
        assert!(HANDOFF.contains("rm -f /etc/init.d/S00arestore"));
        assert!(HANDOFF.contains(FLAG));
        assert!(HANDOFF.contains("ohc_kexec=1"));
    }
}
