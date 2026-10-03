//! SSH transport.
//!
//! Wraps the system `ssh`/`sshpass` behind a small trait. These controllers run
//! ancient sshd (diffie-hellman-group14-sha1, ssh-rsa), which system OpenSSH
//! reaches with the right `-o` options and which a bundled Rust client would
//! have to be specially configured for. OpenSSH ships on macOS, Linux and
//! Windows 10+, so this is portable enough for v1; the trait keeps the door
//! open to a pure-Rust `russh` backend later without the engine noticing.

use std::io::Write;
use std::process::{Command, Stdio};

/// Legacy `-o` options every connection needs, factored out so both `run` and
/// `put_stream` stay in step.
fn legacy_opts() -> Vec<&'static str> {
    vec![
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "LogLevel=ERROR",
        "-o", "ConnectTimeout=8",
        "-o", "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1",
        "-o", "HostKeyAlgorithms=+ssh-rsa",
        "-o", "PubkeyAcceptedAlgorithms=+ssh-rsa",
    ]
}

/// Errors are a plain enum with hand-written `Display`/`Error` — not worth a
/// proc-macro dependency for three variants.
#[derive(Debug)]
pub enum SshError {
    Command { host: String, cmd: String, code: i32, msg: String },
    NoSshpass,
    Spawn(String),
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::Command { host, cmd, code, msg } => {
                write!(f, "{host}: `{cmd}` failed ({code}): {msg}")
            }
            SshError::NoSshpass if cfg!(windows) => write!(
                f,
                "password auth on Windows needs PuTTY's `plink.exe` on PATH (winget install \
                 PuTTY.PuTTY, or choco install putty). `sshpass` is a Unix-only program and has \
                 no Windows build, so the OpenSSH client that ships with Windows cannot be given \
                 a password non-interactively. The alternative is key auth: ssh-keygen, then put \
                 the public key in the unit's authorized_keys."
            ),
            SshError::NoSshpass => write!(
                f,
                "password auth needs `sshpass` on PATH (brew install sshpass, apt install \
                 sshpass), or use a key"
            ),
            SshError::Spawn(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SshError {}

/// A reachable, authenticated box.
#[derive(Clone)]
pub struct Ssh {
    pub host: String,
    pub user: String,
    pub password: Option<String>,
}

impl Ssh {
    pub fn new(host: impl Into<String>, user: impl Into<String>, password: Option<String>) -> Self {
        Ssh { host: host.into(), user: user.into(), password }
    }

    fn base(&self) -> Result<Command, SshError> {
        let mut c;
        if let Some(pw) = &self.password {
            // WINDOWS HAS NO sshpass. It is a Unix program that drives a pty,
            // and Windows' OpenSSH client has no way to be handed a password on
            // the command line at all — which is why every password login
            // failed there while the same build worked on a Mac.
            //
            // PuTTY's plink does take one (`-pw`), and it is the thing Windows
            // users are most likely to already have, so it is the fallback.
            // Its option spelling is entirely different from OpenSSH's, hence
            // the separate argument list rather than a swapped binary name.
            if cfg!(windows) {
                if let Some(plink) = which("plink") {
                    c = Command::new(plink);
                    // -batch: never prompt, fail instead — a prompt here would
                    // hang a GUI with no console attached to answer it.
                    // -no-antispoof: keep the remote output clean for parsing.
                    c.args(["-ssh", "-batch", "-no-antispoof", "-pw", pw, "-l", &self.user, &self.host]);
                    return Ok(c);
                }
                return Err(SshError::NoSshpass);
            }
            if which("sshpass").is_none() {
                return Err(SshError::NoSshpass);
            }
            c = Command::new("sshpass");
            c.arg("-p").arg(pw).arg("ssh");
        } else {
            c = Command::new("ssh");
        }
        c.args(legacy_opts());
        c.arg(format!("{}@{}", self.user, self.host));
        Ok(c)
    }

    /// Run a command; capture stdout. `check` turns a non-zero exit into an
    /// error (some probes want to inspect a failure instead).
    pub fn run(&self, cmd: &str, check: bool) -> Result<String, SshError> {
        let mut last = None;
        for attempt in 0..AUTH_TRIES {
            let mut c = self.base()?;
            c.arg(cmd);
            c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let out = c.output().map_err(|e| SshError::Spawn(e.to_string()))?;
            if spurious_refusal(&out.status, &out.stderr) && attempt + 1 < AUTH_TRIES {
                std::thread::sleep(std::time::Duration::from_millis(700));
                continue;
            }
            last = Some(out);
            break;
        }
        let out = last.expect("loop always sets it");
        if check && !out.status.success() {
            let msg = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
            return Err(SshError::Command {
                host: self.host.clone(),
                cmd: cmd.to_string(),
                code: out.status.code().unwrap_or(-1),
                msg: msg.trim().chars().take(300).collect(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Evaluate a shell `test` expression on the box, e.g. `-s /path`.
    ///
    /// Use this, not `run("test ...", false).is_ok()`: `run` without `check`
    /// returns Ok whatever the exit status, so that idiom is always true. An
    /// unreachable box reads as false.
    pub fn test(&self, expr: &str) -> bool {
        self.run(&format!("test {expr} && echo ohc-yes"), false)
            .map(|o| o.trim() == "ohc-yes")
            .unwrap_or(false)
    }

    /// Cheap reachability + auth probe.
    pub fn ok(&self) -> bool {
        self.run("echo ohc-ok", false).map(|o| o.trim() == "ohc-ok").unwrap_or(false)
    }

    pub fn read_file(&self, path: &str) -> Option<String> {
        let out = self.run(&format!("cat {path} 2>/dev/null"), false).ok()?;
        (!out.trim().is_empty()).then_some(out)
    }

    /// Pipe bytes into a remote command's stdin — how images land, avoiding scp
    /// and any need for free space on a box with a 512 MB rootfs.
    pub fn put_stream(&self, data: &[u8], remote_cmd: &str) -> Result<String, SshError> {
        let mut last: Option<std::process::Output> = None;
        for attempt in 0..AUTH_TRIES {
            let mut c = self.base()?;
            c.arg(remote_cmd);
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = c.spawn().map_err(|e| SshError::Spawn(e.to_string()))?;

            // A refused login makes ssh exit while we are still writing, and the
            // write then fails with EPIPE. Reporting "Broken pipe" would blame
            // the transfer for what is an auth flake, so swallow the write error
            // here and let the child's own exit status and stderr say what
            // actually happened.
            let wrote = child.stdin.take().unwrap().write_all(data);
            let out = child.wait_with_output().map_err(|e| SshError::Spawn(e.to_string()))?;
            if spurious_refusal(&out.status, &out.stderr) && attempt + 1 < AUTH_TRIES {
                std::thread::sleep(std::time::Duration::from_millis(700));
                continue;
            }
            if let Err(e) = wrote {
                if out.status.success() {
                    // The remote consumed less than we sent but still exited 0 —
                    // a truncated file, which is far worse than a failed one.
                    return Err(SshError::Spawn(format!("short write to `{remote_cmd}`: {e}")));
                }
            }
            last = Some(out);
            break;
        }
        let out = last.expect("loop always sets it");
        if !out.status.success() {
            return Err(SshError::Command {
                host: self.host.clone(),
                cmd: remote_cmd.to_string(),
                code: out.status.code().unwrap_or(-1),
                msg: String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// How many times a command is retried through a refused login.
///
/// ROUGHLY ONE AUTH IN TEN IS SPURIOUSLY REFUSED against these controllers.
/// Measured on an HC-800: 47 identical logins produced 5 `Permission denied
/// (publickey, password)` and 42 successes. It is sshpass racing dropbear's
/// password prompt on the pty — the rate is the same with and without the
/// legacy algorithm options, and against both the stock image and openHC.
///
/// A flasher that takes the first refusal at face value therefore fails about
/// one command in ten, and a multi-step install has many commands. Worse, it
/// fails with whatever the symptom happened to be — a 13 MB image transfer dies
/// with "Broken pipe", which reads like a network fault rather than a login
/// that never happened.
const AUTH_TRIES: usize = 3;

/// Did this look like the flake above rather than a real failure? Deliberately
/// narrow: only a login refusal, so a command that genuinely exits non-zero is
/// never quietly run three times.
fn spurious_refusal(status: &std::process::ExitStatus, stderr: &[u8]) -> bool {
    // 255 is ssh's own "I failed", as distinct from the remote command's status.
    if status.code() != Some(255) {
        return false;
    }
    let e = String::from_utf8_lossy(stderr);
    e.contains("Permission denied") || e.contains("Authentication failed")
}

/// Credentials to try, openHC first (a half-installed unit is the common
/// re-run case), then stock Control4.
pub const CANDIDATE_LOGINS: &[(&str, Option<&str>)] = &[
    ("root", Some("openhc")),
    ("openhc", Some("openhc")),
    // Stock Control4, and ONLY UP TO OS 3.0.x. Control4 removed the default
    // root password in OS 3.1.0; a controller on 3.1 or later has no password
    // that will ever work here, no matter how many are tried. Confirmed both
    // ways on hardware: it still works on an HC-800 running the 2.x-era image,
    // and an EA-3 on 3.3.3 refuses it.
    //
    // So a refusal on a modern unit is not a wrong guess, it is the absence of
    // a credential — which is why `first_working_login_with` reports that
    // distinctly instead of saying "no password worked".
    ("root", Some("t0talc0ntr0l4!")),
    ("root", None), // key auth
];

/// First login that answers, or None.
pub fn first_working_login(host: &str) -> Option<Ssh> {
    first_working_login_with(host, &[])
}

/// Like [`first_working_login`], but tries caller-supplied passwords first.
///
/// Newer Control4 firmware (OS 3.1.0+) derives root's password from the unit's
/// MAC, and a dealer may also have set an arbitrary one. The order here is:
///
/// 1. any caller-supplied passwords (a dealer-set one the operator knows);
/// 2. the MAC-derived stock password, computed automatically from the ARP table
///    so an unattended takeover needs no operator input (see [`authderive`]);
/// 3. the known factory and openHC logins.
///
/// A wrong derived guess simply fails and falls through, so this never blocks a
/// box it cannot open — it only removes the hand-typed password on the common
/// case where the box still has its MAC-derived default.
pub fn first_working_login_with(host: &str, passwords: &[String]) -> Option<Ssh> {
    // The calculated/dealer password is root's; try each given one as root.
    for pw in passwords {
        if pw.trim().is_empty() {
            continue;
        }
        let s = Ssh::new(host, "root", Some(pw.clone()));
        if s.ok() {
            return Some(s);
        }
    }
    // The MAC-derived stock root password, resolved with no operator input.
    if let Some(pw) = derived_root_pw_for(host) {
        let s = Ssh::new(host, "root", Some(pw));
        if s.ok() {
            return Some(s);
        }
    }
    for (user, pw) in CANDIDATE_LOGINS {
        let s = Ssh::new(host, *user, pw.map(String::from));
        if s.ok() {
            return Some(s);
        }
    }
    None
}

/// The MAC-derived stock root password for `host`, if its MAC is in the ARP
/// table and belongs to Control4's OUI. Returns `None` when the MAC is unknown
/// (nothing to derive from) or is not a Control4 unit (do not try it elsewhere).
fn derived_root_pw_for(host: &str) -> Option<String> {
    let mac = crate::discovery::arp_table().get(host).cloned()?;
    let m = mac.to_ascii_lowercase();
    if !(m.starts_with("00:0f:ff") || m.starts_with("0:f:ff")) {
        return None;
    }
    crate::authderive::derive_root_pw(&mac)
}

/// Is TCP 22 open on the host? Distinguishes "unreachable" from "reachable but
/// every credential was rejected" — the second means SSH password login is
/// disabled or the password is one we were not given, which is a different
/// message and a different fix for the user.
pub fn ssh_port_open(host: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    (host, 22u16)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .is_some_and(|addr| {
            TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(4)).is_ok()
        })
}

/// Find a program on PATH, returning where it is.
///
/// Returns the path rather than a bare "yes", because the Windows branch needs
/// to actually run what it found. And it tries PATHEXT there: a bare "plink" is
/// not a file on Windows, "plink.exe" is, so a Unix-shaped lookup finds nothing
/// and reports the program as missing when it is installed.
fn which(prog: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.BAT;.CMD".into())
            .split(';')
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![]
    };
    std::env::split_paths(&path).find_map(|dir| {
        let direct = dir.join(prog);
        if direct.is_file() {
            return Some(direct);
        }
        exts.iter().find_map(|e| {
            let p = dir.join(format!("{prog}{e}"));
            p.is_file().then_some(p)
        })
    })
}

/// Poll until the box answers SSH again, or give up after `secs`.
///
/// The initial sleep is load-bearing: a box that has just been told to reboot
/// keeps answering SSH for a few seconds while it shuts down, so polling
/// immediately reconnects to the dying system and the caller believes the
/// reboot already finished.
pub fn wait_for_login(host: &str, secs: u64) -> Option<Ssh> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    std::thread::sleep(std::time::Duration::from_secs(30));
    while std::time::Instant::now() < deadline {
        if let Some(s) = first_working_login(host) {
            return Some(s);
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    None
}
