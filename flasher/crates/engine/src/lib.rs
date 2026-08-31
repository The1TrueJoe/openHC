//! Install *flows*: core decides, transport talks, the engine sequences.
//!
//! Each flow takes a [`transport::Ssh`], a loaded [`release::Release`], and a
//! [`event::Progress`] sink, and drives one stage of an install. The two-stage
//! network flow is split deliberately: stage 1 leaves the box rebooting into a
//! RAM installer, the caller waits for it to return, then runs stage 2. That
//! keeps the "wait for the box" policy in the front end where a user can watch.
pub mod event;
pub mod network;
pub mod updates;
pub mod release;

pub use event::{Event, Progress};
pub use release::Release;
pub use updates::{GhRelease, latest_release};
