//! `ohc-restore watch-button` — hold the front button to return to stock.
//!
//! For boards whose button is an INPUT DEVICE (the HC-800's ID button is claimed
//! by gpio-keys-polled and reports KEY_F5), where the EA's gpioget watcher
//! cannot read the line. Same contract as that watcher
//! (board/ea/common/rootfs-overlay/opt/ohc/bin/ohc-restore-button):
//!
//!   OHC_RESTORE_BUTTON_INPUT  input device name (/proc/bus/input/devices N=)
//!   OHC_RESTORE_BUTTON_KEY    key code (default 63, KEY_F5)
//!   OHC_RESTORE_BUTTON_HOLD   seconds held before restoring (default 10)
//!   OHC_RESTORE_BUTTON_LED    /sys/class/leds entry blinked while arming
//!
//! FAIL-SAFE: it only ARMS after reading the key released at least once, so a
//! stuck button, or one held through boot, never starts a restore. The state is
//! polled (EVIOCGKEY) once a second rather than read from the event stream: the
//! question is "has it been down for N seconds", not "what happened".

use evdev::{Device, KeyCode};
use std::io;
use std::time::Duration;

fn env_or<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(d)
}

fn open(name: &str) -> Option<Device> {
    evdev::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(name))
}

fn led(name: &Option<String>, on: bool) {
    let Some(n) = name else { return };
    let d = format!("/sys/class/leds/{n}");
    // Writing brightness drops a kernel trigger; take control explicitly.
    let _ = std::fs::write(format!("{d}/trigger"), "none");
    let v = if on {
        std::fs::read_to_string(format!("{d}/max_brightness")).unwrap_or_else(|_| "1".into())
    } else {
        "0".into()
    };
    let _ = std::fs::write(format!("{d}/brightness"), v.trim());
}

pub fn watch(stock: fn() -> io::Result<()>) -> io::Result<()> {
    let name = std::env::var("OHC_RESTORE_BUTTON_INPUT").unwrap_or_default();
    if name.is_empty() {
        println!("restore-button: no OHC_RESTORE_BUTTON_INPUT configured");
        return Ok(());
    }
    let key = KeyCode::new(env_or("OHC_RESTORE_BUTTON_KEY", 63u16));
    let hold: u32 = env_or("OHC_RESTORE_BUTTON_HOLD", 10);
    let led_name = std::env::var("OHC_RESTORE_BUTTON_LED").ok().filter(|s| !s.is_empty());
    println!("restore-button: watching '{name}' key {} (hold {hold}s)", key.code());

    let (mut dev, mut armed, mut held, mut warned) = (None::<Device>, false, 0u32, false);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if dev.is_none() {
            dev = open(&name);
            if dev.is_none() {
                continue; // not there (yet); keep looking, never guess
            }
        }
        let pressed = match dev.as_ref().unwrap().get_key_state() {
            Ok(s) => s.contains(key),
            Err(_) => {
                // Device went away: treat as released, reopen next tick.
                dev = None;
                if held > 0 {
                    led(&led_name, false);
                }
                held = 0;
                continue;
            }
        };
        if !pressed {
            armed = true; // a clean released reading: safe to watch
            if held > 0 {
                led(&led_name, false);
            }
            held = 0;
        } else if armed {
            held += 1;
            led(&led_name, held % 2 == 0); // blink while arming
            if held >= hold {
                led(&led_name, true);
                println!("restore-button: held {held}s — returning to stock");
                if let Err(e) = stock() {
                    println!("restore-button: return to stock failed: {e}");
                }
                // If it returns, require a fresh release before arming again.
                (armed, held) = (false, 0);
                led(&led_name, false);
            }
        } else if !warned {
            println!("restore-button: key reads pressed at startup — not arming until it is released");
            warned = true;
        }
    }
}
