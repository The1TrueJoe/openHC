//! Progress events, so a CLI can print them and the GUI can render a live log
//! from the same flow code.

use std::sync::Arc;

/// Severity of a log line, in the Python-`logging` sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
}

impl Level {
    /// Fixed-width tag so columns line up in a terminal.
    pub fn tag(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO ",
            Level::Warn => "WARN ",
        }
    }
}

/// Which logger a line belongs to. Keeping the box's serial output, the
/// commands we type at it, this host's network servers, and the flow's own
/// narration on separate channels is the whole point: you can read "what did I
/// send" without it drowning in "what did the box echo back".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The flow's own narration (steps, details, warnings, resolutions).
    Flow,
    /// Lines the box printed on its serial console (RX).
    Serial,
    /// Commands this tool typed at the box's shell (TX).
    Shell,
    /// This host's BOOTP/TFTP server activity.
    Net,
}

impl Channel {
    /// Fixed-width logger name so columns line up in a terminal.
    pub fn tag(self) -> &'static str {
        match self {
            Channel::Flow => "flow  ",
            Channel::Serial => "serial",
            Channel::Shell => "shell ",
            Channel::Net => "net   ",
        }
    }
}

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
    /// A line the box printed on the serial console — live evidence from the
    /// wire, so a wait does not look like a hang. Rendered on the `serial`
    /// channel. The difference between "nothing is happening" and "we are
    /// watching and have seen nothing yet" is the whole reason this exists.
    Observed(String),
    /// A command this tool typed AT the box's shell (CEFDK or Linux). Rendered
    /// on the `shell` channel so the send side is never confused with the
    /// `serial` lines the box echoes back.
    Shell(String),
    /// Network-server activity on this host — the BOOTP responder and the TFTP
    /// server. Rendered on the `net` channel.
    Net(String),
    /// The awaited physical action happened. Clears any `Await`.
    Resolved(String),
}

impl Event {
    pub fn step(s: String) -> Event { Event::Step(s) }
    pub fn detail(s: String) -> Event { Event::Detail(s) }
    pub fn warn(s: String) -> Event { Event::Warn(s) }
    pub fn observed(s: String) -> Event { Event::Observed(s) }
    pub fn shell(s: String) -> Event { Event::Shell(s) }
    pub fn net(s: String) -> Event { Event::Net(s) }
    pub fn resolved(s: String) -> Event { Event::Resolved(s) }

    /// The log channel and level this event renders on. Front ends that want a
    /// Python-`logging`-style stream (level + named logger + message) read this
    /// instead of matching every variant; `None` means the event is not a log
    /// line (the `Await` banner, which a front end shows as sticky UI).
    pub fn log_fields(&self) -> Option<(Level, Channel, &str)> {
        match self {
            Event::Step(s) => Some((Level::Info, Channel::Flow, s)),
            Event::Detail(s) => Some((Level::Debug, Channel::Flow, s)),
            Event::Warn(s) => Some((Level::Warn, Channel::Flow, s)),
            Event::Observed(s) => Some((Level::Debug, Channel::Serial, s)),
            Event::Shell(s) => Some((Level::Info, Channel::Shell, s)),
            Event::Net(s) => Some((Level::Debug, Channel::Net, s)),
            Event::Resolved(s) => Some((Level::Info, Channel::Flow, s)),
            Event::Await { .. } => None,
        }
    }
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
    /// A sink that prints to stdout — the CLI default. Log lines are rendered
    /// Python-`logging` style, `LEVEL channel | message`, with the level and
    /// the logger name in fixed-width columns so a glance down the left edge
    /// separates serial RX, shell TX, net and flow. The `Await` banner is not a
    /// log line — it is the user's cue to touch the hardware — so it stays loud
    /// and unindented.
    pub fn stdout() -> Progress {
        Progress::new(|e| match e.log_fields() {
            Some((level, channel, msg)) => {
                println!("  {} {} | {msg}", level.tag(), channel.tag());
            }
            None => {
                let Event::Await { title, instruction } = &e else { return };
                println!("\n  ==> ACTION NEEDED: {title}");
                for l in instruction.lines() {
                    println!("      {l}");
                }
                println!();
            }
        })
    }
}
