//! Infrared: the codec between Pronto, which is what an installer pastes, and
//! the microsecond timings [`crate::lirc`] speaks.
//!
//! Easy to get wrong in a way that type-checks: **Pronto counts in CARRIER
//! PERIODS, not microseconds.** Unscaled, a code is well-formed and radiates
//! something no target recognises.
use crate::lirc::{self, Dev};
use crate::ops::{Fault, IrTarget};
use std::sync::Arc;

/// The Pronto clock, `1e6 / 0.241246`. A code's second word is this over the
/// carrier frequency.
pub const PRONTO_HZ: u32 = 4_145_146;

/// Must match `gpio-ohc-iomcu.c` exactly; a mismatch loses the device silently.
pub fn emitter_name(target: IrTarget) -> String {
    match target {
        IrTarget::Front => "openHC IR front blaster".into(),
        IrTarget::Jack(n) => format!("openHC IR out {n}"),
    }
}

pub const RECEIVER_NAME: &str = "openHC IR front receiver";

/// The lirc node for one emitter.
pub fn emitter(target: IrTarget) -> Result<Dev, Fault> {
    let name = emitter_name(target);
    lirc::find(&name).ok_or_else(|| {
        Fault::NoSuch(format!(
            "no lirc device named {name:?} — is gpio-ohc-iomcu attached, and does \
             this board's board.env declare that many IR outputs?"
        ))
    })
}

/// Parse a learned Pronto code into `(carrier Hz, durations in microseconds)`.
pub fn parse_pronto(code: &str) -> Result<(u32, Vec<u32>), Fault> {
    let words: Result<Vec<u16>, _> =
        code.split_whitespace().map(|w| u16::from_str_radix(w, 16)).collect();
    let words = words.map_err(|_| Fault::Bad("pronto must be space-separated hex words".into()))?;
    if words.len() < 5 || words[0] != 0 {
        return Err(Fault::Bad(
            "only Pronto code type 0000 (raw, learned) is supported".into(),
        ));
    }
    // [0000][carrier][once len][repeat len][durations…]
    let word = words[1];
    if word == 0 {
        // The firmware DIVIDES by this. A zero is a UsageFault on the Cortex-M3
        // and the part answers nothing until it is power cycled.
        return Err(Fault::Bad("pronto carrier word cannot be 0000".into()));
    }
    let carrier_hz = PRONTO_HZ / word as u32;
    let durations = &words[4..];
    if durations.is_empty() {
        return Err(Fault::Bad("pronto code carries no burst pairs".into()));
    }
    let us = durations
        .iter()
        .map(|d| ((*d & 0x7fff) as u64 * 1_000_000 / carrier_hz.max(1) as u64) as u32)
        .collect();
    Ok((carrier_hz, us))
}

/// Microsecond marks and spaces back to a Pronto "0000" (learned) code.
pub fn to_pronto(durs: &[u32], carrier_hz: u32) -> String {
    let word = (PRONTO_HZ / carrier_hz.max(1)) as u16;
    let pairs = (durs.len() / 2) as u16;
    let mut out = format!("0000 {word:04X} {pairs:04X} 0000");
    for d in durs {
        let periods = ((*d as u64 * carrier_hz as u64) / 1_000_000).min(0x7fff) as u16;
        out.push_str(&format!(" {periods:04X}"));
    }
    out
}

/// Read the receiver and publish each code as `ir/front/rx`, in the same Pronto
/// form `ir/front/send` accepts — so a learned code can be sent straight back.
///
/// Polled rather than woken: the kernel buffers a whole burst, so a 20 ms tick
/// costs one failed read and keeps this on the single runtime thread.
pub async fn receiver(cfg: Arc<crate::Config>, dev: std::path::PathBuf) {
    use std::io::Read;
    use std::time::Duration;

    let mut f = match lirc::open_rx(&dev) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("iod: cannot open {} for IR receive: {e}", dev.display());
            return;
        }
    };
    eprintln!("iod: IR receive on {}", dev.display());

    let mut durs: Vec<u32> = Vec::new();
    let mut carrier: u32 = 0;
    let mut buf = [0u8; 4096];
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                eprintln!("iod: IR receive stopped: {e}");
                return;
            }
        };
        let (words, _) = buf[..n].as_chunks::<4>();
        for c in words {
            let v = u32::from_ne_bytes(*c);
            let val = v & lirc::VALUE_MASK;
            match v & lirc::MODE2_MASK {
                lirc::MODE2_PULSE => durs.push(val),
                // A space before any mark is the gap since the last code.
                0 if !durs.is_empty() => durs.push(val),
                lirc::MODE2_TIMEOUT => {
                    if !durs.is_empty() {
                        let hz = if carrier > 0 { carrier } else { 38_000 };
                        cfg.bus.event(
                            "ir/front/rx",
                            serde_json::json!({
                                "pronto": to_pronto(&durs, hz),
                                "carrier_hz": hz,
                                "durations": durs.len(),
                            }),
                        );
                    }
                    durs.clear();
                }
                lirc::MODE2_FREQUENCY => carrier = val,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A learned code must go back out unchanged. Getting either scale wrong
    /// yields a code that is well-formed and radiates the wrong thing.
    #[test]
    fn a_learned_code_survives_the_round_trip() {
        let sent = "0000 006D 0002 0000 0062 0017 0018 0017";
        let (hz, us) = parse_pronto(sent).expect("parses");
        assert_eq!(hz, PRONTO_HZ / 0x6d); // 37,957 Hz — a normal remote
        let back = to_pronto(&us, hz);
        let a: Vec<&str> = sent.split_whitespace().collect();
        let b: Vec<&str> = back.split_whitespace().collect();
        assert_eq!(a.len(), b.len(), "{back}");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate().skip(4) {
            let (x, y) = (i64::from_str_radix(x, 16).unwrap(), i64::from_str_radix(y, 16).unwrap());
            // Integer microseconds lose a fraction of a period each way.
            assert!((x - y).abs() <= 1, "word {i}: {x:#06x} vs {y:#06x} in {back}");
        }
    }

    #[test]
    fn refuses_what_cannot_be_replayed() {
        // A zero carrier word is the one that wedges the part.
        assert!(parse_pronto("0000 0000 0002 0000 0062").is_err());
        // Only learned codes; 0100 is a "predefined" code we cannot expand.
        assert!(parse_pronto("0100 006D 0002 0000 0062").is_err());
        assert!(parse_pronto("0000 006D 0000 0000").is_err()); // no durations
        assert!(parse_pronto("not hex").is_err());
    }
}
