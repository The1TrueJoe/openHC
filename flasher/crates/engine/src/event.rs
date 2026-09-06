//! Progress events, so a CLI can print them and the GUI can render a live log
//! from the same flow code.

use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Event {
    /// A headline step the user should see.
    Step(String),
    /// A sub-line (register values, script commands, verifications).
    Detail(String),
    /// Something went wrong but the flow is handling it.
    Warn(String),
    /// The tool cannot continue until the USER does something physical --
    /// hold the ID button, power-cycle the box. This is sticky: front ends
    /// should keep it on screen (a banner, not a log line) until an
    /// `Await`-clearing event arrives, because the user is looking at the
    /// hardware, not the scrollback.
    ///
    /// `title` is the imperative ("Hold the ID button and power-cycle").
    /// `instruction` is the numbered detail.
    Await { title: String, instruction: String },
    /// Live evidence from the wire, so a wait does not look like a hang. The
    /// difference between "nothing is happening" and "we are watching and have
    /// seen nothing yet" is the whole reason this variant exists.
    Observed(String),
    /// The awaited physical action happened. Clears any `Await`.
    Resolved(String),
}

impl Event {
    pub fn step(s: String) -> Event { Event::Step(s) }
    pub fn detail(s: String) -> Event { Event::Detail(s) }
    pub fn warn(s: String) -> Event { Event::Warn(s) }
    pub fn observed(s: String) -> Event { Event::Observed(s) }
    pub fn resolved(s: String) -> Event { Event::Resolved(s) }
    pub fn await_user(title: &str, instruction: &str) -> Event {
        Event::Await { title: title.into(), instruction: instruction.into() }
    }
    /// True when this event ends a pending `Await`, so a front end does not
    /// have to know the variant list to decide whether to drop its banner.
    pub fn clears_await(&self) -> bool {
        matches!(self, Event::Resolved(_) | Event::Step(_))
    }
}

/// A sink for events. The flows call `emit`; the front end supplies the closure.
#[derive(Clone)]
pub struct Progress(Arc<dyn Fn(Event) + Send + Sync>);

impl Progress {
    pub fn new(f: impl Fn(Event) + Send + Sync + 'static) -> Progress {
        Progress(Arc::new(f))
    }
    pub fn emit(&self, e: Event) {
        (self.0)(e)
    }
    /// A sink that prints to stdout — the CLI default.
    pub fn stdout() -> Progress {
        Progress::new(|e| match e {
            Event::Step(s) => println!("  {s}"),
            Event::Detail(s) => println!("    {s}"),
            Event::Warn(s) => println!("  ! {s}"),
            // Loud on purpose: the user is looking at the box, not the terminal,
            // and needs to notice that it is their turn.
            Event::Await { title, instruction } => {
                println!("\n  ==> ACTION NEEDED: {title}");
                for l in instruction.lines() {
                    println!("      {l}");
                }
                println!();
            }
            Event::Observed(s) => println!("    . {s}"),
            Event::Resolved(s) => println!("  OK {s}"),
        })
    }
}
