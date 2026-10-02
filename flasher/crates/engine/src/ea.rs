//! EA return-to-factory over SSH: run the box's own `ohc-restore stock`.
//!
//! That tool (packages/restore) removes openHC's single MFH item from SPI-NOR,
//! read-back verified, then kexecs p2's recovery kernel, which re-images p1 the
//! way the recessed button does. The flasher only starts it: the work, and the
//! MFH safety checks, live on the box next to the driver they need.

use anyhow::{anyhow, bail, Result};
use ohc_flash_transport::ssh::Ssh;

use crate::event::{Event, Progress};

const RESTORE_BIN: &str = "/opt/ohc/bin/ohc-restore";

pub fn restore(ssh: &Ssh, p: &Progress) -> Result<()> {
    if ssh.run(&format!("test -x {RESTORE_BIN}"), false).is_err() {
        if ssh.read_file("/opt/ohc/board.env").is_none() {
            bail!("this box is running stock Control4; there is nothing of openHC's boot to remove from here");
        }
        bail!("this openHC has no {RESTORE_BIN}; install a newer image first");
    }
    let st = ssh.run(&format!("{RESTORE_BIN} status"), true).map_err(|e| anyhow!("{e}"))?;
    p.emit(Event::detail(st.trim().to_string()));
    p.emit(Event::step("removing openHC's MFH item and handing over to Control4's recovery".into()));
    // Detached: the recovery kexec takes the box (and this connection) down.
    let _ = ssh.run(&format!("(nohup {RESTORE_BIN} stock >/dev/null 2>&1 &)"), false);
    p.emit(Event::detail("the box re-images p1 from p2 and comes back as stock Control4".into()));
    Ok(())
}
