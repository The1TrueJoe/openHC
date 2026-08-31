//! Turn an SSH connection into a board [`Identity`].
//!
//! openHC is checked first (a half-installed unit being re-run is the common
//! case), then stock Control4's `/proc/c4board`.

use ohc_flash_core::board::{self, Identity, Running};

use crate::ssh::Ssh;

pub fn identify(ssh: &Ssh) -> Identity {
    // openHC: /opt/ohc/board.env
    if let Some(env) = ssh.read_file("/opt/ohc/board.env") {
        let get = |key: &str| env_value(&env, key);
        let id = board::from_board_env(get);
        if id.board.is_some() || !id.candidates.is_empty() {
            return id;
        }
    }
    // stock: /proc/c4board/{name,type,revision}
    let name = ssh.read_file("/proc/c4board/name");
    let btype = ssh.read_file("/proc/c4board/type").and_then(|s| s.trim().parse().ok());
    let rev = ssh.read_file("/proc/c4board/revision").and_then(|s| s.trim().parse().ok());
    if name.is_some() || btype.is_some() || rev.is_some() {
        return board::from_c4board(name.as_deref().map(str::trim), btype, rev);
    }
    Identity { board: None, candidates: vec![], running: Running::Unknown, raw: vec![] }
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
