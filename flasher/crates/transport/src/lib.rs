//! How the flasher talks to a box: SSH, network discovery, and (later) serial.
//!
//! All I/O lives here so `core` stays pure. Backends sit behind small types so
//! a pure-Rust SSH client can replace the system-`ssh` wrapper later without
//! the engine changing.

pub mod discovery;
pub mod sddp;
pub mod probe;
pub mod ssh;

pub use discovery::{discover, Found};
pub use sddp::{search as sddp_search, SddpUnit};
pub use probe::identify;
pub use ssh::{first_working_login, first_working_login_with, ssh_port_open, wait_for_login, Ssh, SshError};
