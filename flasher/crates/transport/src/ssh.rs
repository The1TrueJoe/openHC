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
            SshError::NoSshpass => write!(
                f,
                "password auth needs `sshpass` on PATH (brew/apt install sshpass), or use a key"
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
    ("root", Some("t0talc0ntr0l4!")), // stock Control4 (confirmed on an EA3)
    ("root", None),                   // key auth
];

/// First login that answers, or None.
pub fn first_working_login(host: &str) -> Option<Ssh> {
    first_working_login_with(host, &[])
}

/// Like [`first_working_login`], but tries caller-supplied passwords first.
///
/// This is how the "calculated" or dealer-set root password is handled: newer
/// Control4 firmware derives root's password from the unit's MAC (an algorithm
/// Control4 has never published and this tool will not guess), and a dealer may
/// have set an arbitrary one. Rather than ship a wrong guess that silently fails
/// to authenticate, the front end asks the operator for it and passes it here,
/// ahead of the known factory and openHC logins.
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
    for (user, pw) in CANDIDATE_LOGINS {
        let s = Ssh::new(host, *user, pw.map(String::from));
        if s.ok() {
            return Some(s);
        }
    }
    None
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

fn which(prog: &str) -> Option<()> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let p = dir.join(prog);
        p.is_file().then_some(())
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
