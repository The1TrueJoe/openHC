//! How the flasher talks to a box: SSH, network discovery, serial, and the
//! netboot bring-up servers (TFTP + the C4_COOKIE BOOTP responder).
//!
//! All I/O lives here so `core` stays pure. Backends sit behind small types so
//! a pure-Rust SSH client can replace the system-`ssh` wrapper later without
//! the engine changing.

pub mod bootp;
pub mod discovery;
pub mod probe;
pub mod sddp;
pub mod serial;
pub mod ssh;
pub mod tftp;

pub use bootp::{build_cookie_reply, parse_mac, BootpResponder};
pub use discovery::{discover, Found};
pub use probe::identify;
pub use sddp::{search as sddp_search, SddpUnit};
pub use serial::{Serial, SerialError, CEFDK_BAUD};
pub use ssh::{first_working_login, first_working_login_with, ssh_port_open, wait_for_login, Ssh, SshError};
pub use tftp::TftpServer;
