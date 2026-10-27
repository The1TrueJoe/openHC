//! The Zigbee radio's half of a return to stock: put Control4's own NCP firmware
//! back before anything else changes, so the box Control4 gets back looks like
//! openHC never ran on it. The work is zigbee's (`--restore-stock`, see
//! packages/zigbee/src/stock.rs); this only stops the service that holds the port
//! and runs it with the board's settings.
//!
//! It runs first because it is the step that can fail: on failure nothing else
//! has been touched yet, the service is started again and the restore stops.
//! `--keep-radio` skips it.

use std::io;
use std::path::Path;
use std::process::Command;

const ZIGBEE: &str = "/opt/ohc/bin/zigbee";
const SERVICE: &str = "/etc/init.d/S91ohczigbee";

pub fn restore() -> io::Result<()> {
    if !Path::new(ZIGBEE).exists() {
        return Ok(()); // no zigbee feature on this board
    }
    println!("radio: putting Control4's Zigbee firmware back (stops the zigbee service)");
    let _ = Command::new(SERVICE).arg("stop").status();
    let _ = Command::new("killall").arg("zigbee").status();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let st = Command::new("sh")
        .args(["-c", &format!("set -a; . /opt/ohc/board.env; set +a; exec {ZIGBEE} --restore-stock")])
        .status()?;
    if st.success() {
        return Ok(());
    }
    let _ = Command::new(SERVICE).arg("start").status();
    Err(io::Error::other(
        "the Zigbee radio is not back on Control4's firmware (see above); nothing else was changed. \
         `--keep-radio` returns to stock without it",
    ))
}
