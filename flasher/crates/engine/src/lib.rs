//! Install *flows*: core decides, transport talks, the engine sequences.
//!
//! Each flow takes a [`transport::Ssh`], a loaded [`release::Release`], and a
//! [`event::Progress`] sink, and drives one stage of an install. The two-stage
//! network flow is split deliberately: stage 1 leaves the box rebooting into a
//! RAM installer, the caller waits for it to return, then runs stage 2. That
//! keeps the "wait for the box" policy in the front end where a user can watch.
pub mod event;
pub mod ca1;
pub mod ea;
pub mod hc800;
pub mod iox;
pub mod mfgmode;
pub mod netboot;
pub mod network;
pub mod radio;
pub mod updates;
pub mod release;

pub use event::{Event, Progress};
pub use mfgmode::{MfgWatch, Stage};
pub use netboot::netboot;
pub use release::Release;
pub use updates::{GhRelease, latest_release};

/// The box's own return-to-stock tool (packages/restore), present on an openHC
/// built with the `restore` feature. Images from before the rename install it as
/// `ohc-restore`, so look for the current name first and fall back to the old one.
const RESTORE_BINS: [&str; 2] = ["/opt/ohc/bin/restore", "/opt/ohc/bin/ohc-restore"];

/// Where this box keeps its restore tool, or `None` if it has none.
pub(crate) fn restore_bin(ssh: &ohc_flash_transport::ssh::Ssh) -> Option<&'static str> {
    RESTORE_BINS.into_iter().find(|b| ssh.test(&format!("-x {b}")))
}
