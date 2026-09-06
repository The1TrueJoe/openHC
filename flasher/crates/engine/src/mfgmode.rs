//! Watching an EA into CEFDK manufacturing mode, and saying so out loud.
//!
//! The serial install needs the recessed ID button held through a power cycle.
//! That is the one step this tool cannot do for the user, so it is the one step
//! worth narrating properly: what to do, and — just as important — what the
//! tool is seeing while it waits. A silent wait is indistinguishable from a
//! hang, and the failure modes here all look identical from the outside.
//!
//! This is a PURE state machine on purpose. It takes console lines in and gives
//! events out; it opens no port and sleeps on nothing. That is what makes the
//! interesting cases testable without a board on the bench, and every one of
//! them below was met on real hardware:
//!
//!   * the button was not actually held        -> `Manufacturing Mode: Disabled`
//!   * the DHCP cookie was malformed           -> CEFDK reboots via `AC_BOOT`
//!   * a bare `shell>` is NOT a shell          -> it is the autoscript echoing
//!
//! That last one cost a wasted power cycle: matching on `shell>` reported "at
//! the CEFDK shell" while the box was actually running its stored autoscript,
//! so every command typed afterwards went nowhere.

use crate::{Event, Progress};

/// Where the box is in the manufacturing-mode sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Nothing seen yet. The box is off, or booting normally without us.
    WaitingForPowerCycle,
    /// CEFDK printed its mode line with the button NOT held. The box will boot
    /// normally; it needs another try.
    ButtonNotHeld,
    /// Manufacturing mode confirmed. CEFDK is now looking for a BOOTP server.
    WaitingForCookie,
    /// Our cookie was accepted; CEFDK is dropping to its shell.
    WaitingForShell,
    /// At the CEFDK shell, ready to be driven.
    AtShell,
    /// CEFDK rejected the cookie and took its normal boot path instead.
    CookieRejected,
}

pub const HOLD_INSTRUCTION: &str = "\
1. Unplug power from the EA.
2. Press and HOLD the recessed ID button.
3. Plug power back in, still holding.
4. Keep holding until this window says manufacturing mode is enabled (~10 s).";

/// Consumes console lines and reports progress. Feed it every line; it decides
/// what matters.
#[derive(Debug)]
pub struct MfgWatch {
    stage: Stage,
    /// Lines seen since the last stage change, so a wait can show evidence
    /// rather than a blank screen.
    quiet_lines: usize,
}

impl Default for MfgWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl MfgWatch {
    pub fn new() -> Self {
        MfgWatch { stage: Stage::WaitingForPowerCycle, quiet_lines: 0 }
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// The opening instruction. Emitted once, before any waiting.
    pub fn prompt(&self, p: &Progress) {
        p.emit(Event::await_user(
            "Hold the ID button and power-cycle the EA",
            HOLD_INSTRUCTION,
        ));
        p.emit(Event::detail(
            "watching the console — every line the box prints will appear below".into(),
        ));
    }

    /// Feed one console line. Returns the new stage if it changed.
    pub fn feed(&mut self, line: &str, p: &Progress) -> Option<Stage> {
        let l = line.trim();
        if l.is_empty() {
            return None;
        }
        self.quiet_lines += 1;

        // Order matters: the specific "Enabled"/"Disabled" test must run before
        // the generic mode-line test, or a disabled button reads as progress.
        if l.contains("Manufacturing Mode:") {
            return if l.contains("Enabled") {
                p.emit(Event::resolved("manufacturing mode enabled — button was held".into()));
                self.to(Stage::WaitingForCookie, p)
            } else {
                // Not a failure of the tool; the user simply has to try again,
                // and saying which half went wrong saves a guess.
                p.emit(Event::warn(format!("the box reported: {l}")));
                p.emit(Event::await_user(
                    "Button was not registered — try again",
                    "The box powered up but did not see the ID button held.\n\
                     Hold it BEFORE applying power and keep holding for ~10 s after.\n\
                     Unplug, hold the button, plug in, keep holding.",
                ));
                self.to(Stage::ButtonNotHeld, p)
            };
        }

        // CEFDK announces this only once it has accepted a BOOTP reply carrying
        // the Control4 cookie. It is the real confirmation that our DHCP server
        // answered correctly.
        if l.contains("Entering Control4 Manufacturing Mode") {
            p.emit(Event::resolved("CEFDK accepted our boot cookie".into()));
            return self.to(Stage::WaitingForShell, p);
        }

        // The cookie must be NUL-padded; an END byte straight after option 60
        // makes CEFDK read "C4_COOKIE\xff", reject it, and take AC_BOOT instead.
        if l.contains("AC_BOOT") && self.stage == Stage::WaitingForCookie {
            p.emit(Event::warn(
                "CEFDK took its normal boot path (AC_BOOT) — the cookie was refused".into(),
            ));
            return self.to(Stage::CookieRejected, p);
        }

        // Only trust a prompt once the cookie has been accepted. Before that a
        // `shell>` on the wire is the stored autoscript echoing its own
        // commands, and treating it as a prompt loses a power cycle.
        if l.contains("shell>") && self.stage == Stage::WaitingForShell {
            p.emit(Event::resolved("CEFDK shell is up — taking over".into()));
            return self.to(Stage::AtShell, p);
        }

        // Not a milestone, but proof of life. Front ends render these small.
        p.emit(Event::observed(l.chars().take(120).collect::<String>()));
        None
    }

    /// Called periodically by the driver when nothing has arrived, so a silent
    /// port is reported as silence rather than as a stall.
    pub fn tick(&self, elapsed_s: u64, p: &Progress) {
        if self.stage != Stage::WaitingForPowerCycle || self.quiet_lines > 0 {
            return;
        }
        // 45 s is comfortably past a normal CEFDK banner, so silence this long
        // means the cable or the power, not slowness.
        if elapsed_s > 0 && elapsed_s % 45 == 0 {
            p.emit(Event::detail(format!(
                "{elapsed_s}s: still nothing on the console. Check the serial cable and \
                 that the adapter is the one this tool opened; the box prints its banner \
                 at 115200 within a second or two of power-on."
            )));
        }
    }

    fn to(&mut self, s: Stage, p: &Progress) -> Option<Stage> {
        if self.stage == s {
            return None;
        }
        self.stage = s;
        self.quiet_lines = 0;
        if let Some(next) = next_action(s) {
            p.emit(Event::step(next.into()));
        }
        Some(s)
    }
}

/// What the tool does next, phrased for someone watching the box.
fn next_action(s: Stage) -> Option<&'static str> {
    match s {
        Stage::WaitingForCookie => {
            Some("answering the box's BOOTP request with the Control4 boot cookie")
        }
        Stage::WaitingForShell => Some("waiting for the CEFDK shell prompt"),
        Stage::AtShell => Some("at the CEFDK shell — starting the transfer"),
        Stage::CookieRejected => Some("re-arming — power-cycle with the button held to retry"),
        Stage::ButtonNotHeld => None, // the Await above already says it
        Stage::WaitingForPowerCycle => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recorder() -> (Progress, Arc<Mutex<Vec<Event>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        (Progress::new(move |e| l.lock().unwrap().push(e)), log)
    }

    fn awaits(log: &Arc<Mutex<Vec<Event>>>) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Await { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn happy_path_reaches_the_shell() {
        let (p, _log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("CEFDK 1.2.3 booting", &p);
        w.feed("Manufacturing Mode: Enabled", &p);
        assert_eq!(w.stage(), Stage::WaitingForCookie);
        w.feed("Entering Control4 Manufacturing Mode", &p);
        assert_eq!(w.stage(), Stage::WaitingForShell);
        w.feed("shell>", &p);
        assert_eq!(w.stage(), Stage::AtShell);
    }

    /// The failure that wasted a power cycle: `shell>` appears while the stored
    /// autoscript echoes its commands, long before any shell exists.
    #[test]
    fn a_bare_shell_prompt_before_the_cookie_is_not_a_shell() {
        let (p, _log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("shell>", &p);
        w.feed("shell> emmc rd 0x800400 0x6000000 0x561000", &p);
        assert_eq!(
            w.stage(),
            Stage::WaitingForPowerCycle,
            "autoscript echo must not be mistaken for a prompt"
        );
    }

    /// Reporting "waiting..." when the button was demonstrably not held is the
    /// difference between a five-second retry and ten minutes of confusion.
    #[test]
    fn a_disabled_mode_line_tells_the_user_to_retry() {
        let (p, log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("Manufacturing Mode: Disabled", &p);
        assert_eq!(w.stage(), Stage::ButtonNotHeld);
        let a = awaits(&log);
        assert_eq!(a.len(), 1);
        assert!(a[0].contains("not registered"), "got {:?}", a);
    }

    #[test]
    fn ac_boot_after_manufacturing_mode_is_reported_as_a_refused_cookie() {
        let (p, _log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("Manufacturing Mode: Enabled", &p);
        w.feed("AC_BOOT: booting from eMMC", &p);
        assert_eq!(w.stage(), Stage::CookieRejected);
    }

    /// AC_BOOT is normal chatter on an ordinary boot; only treat it as a
    /// rejection once we were actually expecting our cookie to be taken.
    #[test]
    fn ac_boot_before_manufacturing_mode_is_just_a_normal_boot() {
        let (p, _log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("AC_BOOT: booting from eMMC", &p);
        assert_eq!(w.stage(), Stage::WaitingForPowerCycle);
    }

    #[test]
    fn ordinary_lines_are_surfaced_as_evidence_not_dropped() {
        let (p, log) = recorder();
        let mut w = MfgWatch::new();
        w.feed("CEFDK version 1.2.3", &p);
        let seen = log
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Observed(s) if s.contains("CEFDK version")));
        assert!(seen, "a waiting user must see that lines are arriving");
    }

    #[test]
    fn silence_is_reported_with_something_actionable() {
        let (p, log) = recorder();
        let w = MfgWatch::new();
        w.tick(45, &p);
        let said = log
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Detail(s) if s.contains("serial cable")));
        assert!(said, "a silent port should name the likely cause");
    }
}
