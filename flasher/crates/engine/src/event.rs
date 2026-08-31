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
}

impl Event {
    pub fn step(s: String) -> Event { Event::Step(s) }
    pub fn detail(s: String) -> Event { Event::Detail(s) }
    pub fn warn(s: String) -> Event { Event::Warn(s) }
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
        })
    }
}
