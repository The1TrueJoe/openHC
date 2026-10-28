//! The Zigbee radio's half of a return to stock, over SSH: run the box's own
//! `ohc-zigbee --restore-stock` so Control4 gets its NCP firmware back as it
//! shipped (packages/zigbee/src/stock.rs does the work, from the recovery
//! partition's factory image). Every return to stock runs this FIRST, in the
//! foreground, so a radio that cannot be put back stops the restore before
//! anything else is touched and the reason is on screen.

use anyhow::{anyhow, Result};
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};

const ZIGBEE: &str = "/opt/ohc/bin/ohc-zigbee";
const SERVICE: &str = "/etc/init.d/S91ohczigbee";

pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    if !ssh.test(&format!("-x {ZIGBEE}")) {
        return Ok(()); // no radio on this board, or stock already
    }
    // An ohc-zigbee from before --restore-stock would take the flag for a normal
    // start and serve the port forever, so ask the binary first.
    if ssh.run(&format!("grep -q -- --restore-stock {ZIGBEE}"), true).is_err() {
        p.emit(Event::warn(
            "this openHC's ohc-zigbee cannot put Control4's radio firmware back; the radio keeps \
             openHC's (Control4 reflashes its own once its Zigbee service runs)"
                .into(),
        ));
        return Ok(());
    }
    p.emit(Event::step("putting Control4's Zigbee firmware back on the radio (~2 min)".into()));
    let out = ssh.run(
        &format!(
            "{SERVICE} stop >/dev/null 2>&1; killall ohc-zigbee 2>/dev/null; sleep 1; \
             set -a; . /opt/ohc/board.env; set +a; {ZIGBEE} --restore-stock 2>&1"
        ),
        true,
    );
    match out {
        Ok(o) => {
            for l in o.lines().filter(|l| !l.trim().is_empty() && !l.contains('%')) {
                p.emit(Event::detail(l.trim().to_string()));
            }
            Ok(())
        }
        Err(e) => {
            let _ = ssh.run(&format!("{SERVICE} start >/dev/null 2>&1"), false);
            Err(anyhow!("the radio is not back on Control4's firmware, so nothing else was changed: {e}"))
        }
    }
}
