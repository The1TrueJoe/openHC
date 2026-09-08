//! The event stream: things that change on their own.
//!
//! A contact closing is not a request/response — nobody asked. The MCU protocol
//! has no unsolicited push for it either (the host POLLS, and the firmware's
//! own opcode sweep shows no notify path), so iod polls on the client's behalf
//! and publishes only TRANSITIONS. Clients then get "contact 2 closed" instead
//! of four states four times a second.
//!
//! This is what makes "use a contact to detect state" work: a door sensor is
//! interesting at the moment it changes, and a UI that has to poll to notice is
//! a UI that misses a doorbell between frames.
use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A contact input changed. `closed` is true when the circuit is made —
    /// bit N of the MCU's u32 mask, where 1 = CLOSED.
    Contact { index: u8, closed: bool },
    /// The whole contact mask, sent once when a client connects so it starts
    /// with the truth rather than waiting for the first change.
    ContactSnapshot { mask: u32, closed: Vec<bool> },
    /// The MCU stopped answering, or started again. Worth surfacing: every
    /// other reading becomes meaningless while this is false.
    McuLink { up: bool },
}

#[derive(Clone)]
pub struct Bus(broadcast::Sender<Event>);

impl Bus {
    pub fn new() -> Bus {
        // Bounded on purpose. A client that stops reading must not grow this
        // without limit; broadcast drops the oldest for that receiver and tells
        // it how many it missed, which is the right failure for a live view.
        let (tx, _) = broadcast::channel(64);
        Bus(tx)
    }
    pub fn publish(&self, e: Event) {
        // Err just means nobody is listening.
        let _ = self.0.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
}
