//! Turn an SSH connection into a board [`Identity`].
//!
//! openHC is checked first (a half-installed unit being re-run is the common
//! case), then stock Control4's `/proc/c4board`, then SMBIOS.
//!
//! SMBIOS is last because only one board has it — but for that board it is the
//! ONLY route. The HC-800 is a PC with an AMI BIOS and no `/proc/c4board` at
//! all, so a stock unit answers neither of the first two questions and used to
//! come back `Unknown`.

use ohc_flash_core::board::{self, Identity, Running};

use crate::ssh::Ssh;

/// What the unit is RUNNING, asked for directly.
///
/// Control4 keeps its OS version in a dpkg package rather than any of the usual
/// files — there is no /etc/*version* on these images at all, which is why this
/// is a package query and not a `cat`. Measured on an HC-800:
///
/// ```text
/// # dpkg-query -W -f='${Version}' system-version
/// 3.3.0.628678-res
/// ```
///
/// openHC writes its own build into /etc/openhc-release. One round trip asks
/// for both, because every SSH connection to these boxes is another roll
/// against a login that fails about one time in ten.
fn version_of(ssh: &Ssh) -> Option<String> {
    let out = ssh
        .run(
            "sed -n 's/^version=//p' /etc/openhc-release 2>/dev/null;              dpkg-query -W -f='${Version}' system-version 2>/dev/null",
            false,
        )
        .ok()?;
    let v = out.split_whitespace().next()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

pub fn identify(ssh: &Ssh) -> Identity {
    // openHC: /opt/ohc/board.env
    if let Some(env) = ssh.read_file("/opt/ohc/board.env") {
        let get = |key: &str| env_value(&env, key);
        let mut id = board::from_board_env(get);
        if id.board.is_some() || !id.candidates.is_empty() {
            id.version = version_of(ssh);
            return id;
        }
    }
    // stock: /proc/c4board/{name,type,revision}
    let name = ssh.read_file("/proc/c4board/name");
    let btype = ssh.read_file("/proc/c4board/type").and_then(|s| s.trim().parse().ok());
    let rev = ssh.read_file("/proc/c4board/revision").and_then(|s| s.trim().parse().ok());
    let stock = name.is_some() || btype.is_some() || rev.is_some();
    if stock {
        let mut id = board::from_c4board(name.as_deref().map(str::trim), btype, rev);
        if id.board.is_some() || !id.candidates.is_empty() {
            id.version = version_of(ssh);
            return id;
        }
    }

    // SMBIOS. `running` is decided by what we found above, not by DMI, which
    // says what the HARDWARE is and nothing about what booted on it.
    let vendor = ssh.read_file("/sys/class/dmi/id/sys_vendor");
    let product = ssh.read_file("/sys/class/dmi/id/product_name");
    if vendor.is_some() || product.is_some() {
        let running = if ssh.read_file("/opt/ohc/board.env").is_some() {
            Running::Openhc
        } else if stock {
            Running::Stock
        } else {
            Running::Unknown
        };
        let mut id = board::from_dmi(
            vendor.as_deref().map(str::trim),
            product.as_deref().map(str::trim),
            running,
        );
        if id.board.is_some() {
            id.version = version_of(ssh);
            return id;
        }
    }
    Identity { board: None, candidates: vec![], running: Running::Unknown, version: version_of(ssh), raw: vec![] }
}

/// Pull `KEY="value"` / `KEY=value` out of a board.env blob.
fn env_value(env: &str, key: &str) -> Option<String> {
    for line in env.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                let val = val.trim().trim_matches('"');
                let val = val.split('#').next().unwrap_or(val).trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}
