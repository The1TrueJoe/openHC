//! Board identity: what a unit *is*, independent of what is running on it.
//!
//! A unit must be identifiable from every side of a takeover — running stock
//! Control4 (`/proc/c4board`), running openHC (`/opt/ohc/board.env`), or sitting
//! at the CEFDK shell with no OS (the banner's `Type N, Rev M`). The type and
//! revision pairs here are MEASURED against real units and recorded in
//! `docs/<board>-recon.md`; where a variant has not been read off hardware its
//! ids are `None`, and detection then reports an honest ambiguity instead of
//! guessing — a wrong guess flashes the wrong image at someone's board.

use serde::{Deserialize, Serialize};

/// SoC family. Determines which install *methods* even apply: the EA boards use
/// CEFDK + an eMMC container, the CA-1 uses U-Boot + a boot.scr, and they must
/// never be confused for one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    /// Intel CE5300 (EA1/EA3) — CEFDK, kernel in a raw eMMC container.
    Ea,
    /// Freescale i.MX6SL (CA-1) — U-Boot from SPI-NOR, boot.scr on vfat.
    Ca,
    /// TI DM355 (IOX).
    Iox,
    /// Atom D525 (HC800) — a PC, really.
    Hc,
}

/// A model openHC can run on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    /// openHC board directory, e.g. `ea3-v2`.
    pub name: &'static str,
    pub family: Family,
    pub desc: &'static str,
    /// `/proc/c4board/type`, when a unit has been read.
    pub c4_type: Option<u8>,
    /// `/proc/c4board/revision` values seen on real units.
    pub c4_revs: &'static [u8],
    /// Fuse blown -> CEFDK's verifying `bootkernel` rejects an unsigned image,
    /// so the install must route through the autoscript + `bootlinux` bypass.
    pub secure_boot: bool,
    pub has_switch: bool,
    pub has_wifi: bool,
    pub notes: &'static str,
}

impl Board {
    /// The single fact the installer branches on: a secure-boot part cannot use
    /// the normal verifying boot path, so it needs the autoscript.
    pub fn needs_autoscript(&self) -> bool {
        self.secure_boot
    }
}

/// The known-board table. Static and exhaustive; add ids as units are read, and
/// never fill one in speculatively.
pub const BOARDS: &[Board] = &[
    Board {
        name: "ea1-v1",
        family: Family::Ea,
        desc: "EA1 (board v1, Wi-Fi, no switch)",
        c4_type: Some(1),
        c4_revs: &[5],
        secure_boot: false,
        has_switch: false,
        has_wifi: true,
        notes: "fuse clear: an unsigned container at 0x400 boots, so no autoscript is needed.",
    },
    Board {
        name: "ea1-v2",
        family: Family::Ea,
        desc: "EA1 (board v2, Wi-Fi, no switch)",
        c4_type: Some(1),
        c4_revs: &[],
        secure_boot: false,
        has_switch: false,
        has_wifi: true,
        notes: "ids not yet read off hardware; boot behaviour assumed to match v1 and NOT verified.",
    },
    Board {
        name: "ea1-v2-poe",
        family: Family::Ea,
        desc: "EA1 v2 PoE (switch, no Wi-Fi)",
        c4_type: Some(1),
        c4_revs: &[],
        secure_boot: false,
        has_switch: true,
        has_wifi: false,
        notes: "CEFDK's own enum calls this board_ea1p = 5, which is not /proc/c4board/type.",
    },
    Board {
        name: "ea3-v1",
        family: Family::Ea,
        desc: "EA3 v1 (Wi-Fi, switch, PoE)",
        c4_type: Some(2),
        c4_revs: &[],
        secure_boot: true,
        has_switch: true,
        has_wifi: true,
        notes: "",
    },
    Board {
        name: "ea3-v2",
        family: Family::Ea,
        desc: "EA3 v2 (switch, PoE, no Wi-Fi)",
        c4_type: Some(2),
        c4_revs: &[9],
        secure_boot: true,
        has_switch: true,
        has_wifi: false,
        notes: "secure-boot fuse BLOWN (measured): bootkernel rejects unsigned images, so this \
                board must install via the autoscript + bootlinux path.",
    },
    Board {
        name: "ca1",
        family: Family::Ca,
        desc: "CA-1 (i.MX6SL, U-Boot)",
        c4_type: Some(0),
        c4_revs: &[4],
        secure_boot: false,
        has_switch: false,
        has_wifi: false,
        notes: "stock bootcmd already tries boot.scr on the vfat partition; dropping one takes \
                over and deleting it reverts.",
    },
    Board {
        name: "ioxv1",
        family: Family::Iox,
        desc: "IOX v1 (TI DM355)",
        c4_type: None,
        c4_revs: &[],
        secure_boot: false,
        has_switch: false,
        has_wifi: false,
        notes: "",
    },
    Board {
        name: "hc800",
        family: Family::Hc,
        desc: "HC800 (Atom D525)",
        c4_type: None,
        c4_revs: &[],
        secure_boot: false,
        has_switch: false,
        has_wifi: false,
        notes: "",
    },
];

/// Look a board up by its openHC name (`ea3-v2`).
pub fn by_name(name: &str) -> Option<&'static Board> {
    let name = name.trim().to_ascii_lowercase();
    BOARDS.iter().find(|b| b.name == name)
}

fn by_type(t: u8) -> Vec<&'static Board> {
    BOARDS.iter().filter(|b| b.c4_type == Some(t)).collect()
}

/// Where the identification came from — the app shows this so a user knows how
/// much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Running {
    Stock,
    Openhc,
    Cefdk,
    Unknown,
}

/// The result of trying to work out what a unit is, and how sure we are.
#[derive(Debug, Clone)]
pub struct Identity {
    pub board: Option<&'static Board>,
    pub candidates: Vec<&'static Board>,
    pub running: Running,
    /// Raw evidence, for display.
    pub raw: Vec<(String, String)>,
}

impl Identity {
    fn none(running: Running) -> Self {
        Identity { board: None, candidates: vec![], running, raw: vec![] }
    }

    /// True only when exactly one board matches — the precondition for an
    /// unattended install.
    pub fn certain(&self) -> bool {
        self.board.is_some() && self.candidates.len() <= 1
    }

    pub fn describe(&self) -> String {
        match (&self.board, self.certain()) {
            (Some(b), true) => format!("{} ({}) running {:?}", b.name, b.desc, self.running),
            _ if !self.candidates.is_empty() => {
                let names: Vec<_> = self.candidates.iter().map(|b| b.name).collect();
                format!("ambiguous: could be {} (running {:?})", names.join(", "), self.running)
            }
            _ => format!("unidentified board (running {:?})", self.running),
        }
    }
}

/// Identify from a stock Control4 `/proc/c4board`.
pub fn from_c4board(name: Option<&str>, btype: Option<u8>, rev: Option<u8>) -> Identity {
    let mut raw = vec![];
    if let Some(n) = name {
        raw.push(("name".into(), n.to_string()));
    }
    if let Some(t) = btype {
        raw.push(("type".into(), t.to_string()));
    }
    if let Some(r) = rev {
        raw.push(("revision".into(), r.to_string()));
    }

    // /proc/c4board/name is a short family tag (`ea3`), not the variant, so it
    // rarely matches a board name directly; fall through to type+revision.
    if let Some(n) = name {
        if let Some(b) = by_name(n) {
            return Identity { board: Some(b), candidates: vec![b], running: Running::Stock, raw };
        }
    }
    let pool = btype.map(by_type).unwrap_or_default();
    let exact: Vec<_> = pool
        .iter()
        .copied()
        .filter(|b| rev.is_some_and(|r| b.c4_revs.contains(&r)))
        .collect();
    let board = if exact.len() == 1 {
        Some(exact[0])
    } else if pool.len() == 1 {
        Some(pool[0])
    } else {
        None
    };
    let candidates = if exact.len() == 1 { exact } else { pool };
    Identity { board, candidates, running: Running::Stock, raw }
}

/// Identify from a running openHC `/opt/ohc/board.env` (already parsed).
pub fn from_board_env(get: impl Fn(&str) -> Option<String>) -> Identity {
    let mut raw = vec![];
    if let Some(name) = get("OHC_BOARD") {
        raw.push(("OHC_BOARD".into(), name.clone()));
        if let Some(b) = by_name(&name) {
            return Identity { board: Some(b), candidates: vec![b], running: Running::Openhc, raw };
        }
    }
    // Older overlays carry only OHC_MODEL (1/3/5), i.e. the family not the variant.
    if let Some(model) = get("OHC_MODEL") {
        raw.push(("OHC_MODEL".into(), model.clone()));
        let prefix = format!("ea{}", model.trim());
        let pool: Vec<_> = BOARDS
            .iter()
            .filter(|b| b.family == Family::Ea && b.name.starts_with(&prefix))
            .collect();
        let board = if pool.len() == 1 { Some(pool[0]) } else { None };
        return Identity { board, candidates: pool, running: Running::Openhc, raw };
    }
    Identity::none(Running::Openhc)
}

/// Identify from CEFDK's own banner text, e.g. `Board : Type 1, Rev 5`.
pub fn from_cefdk_banner(text: &str) -> Identity {
    // A tiny hand parser rather than pulling in `regex` for one pattern.
    let find_after = |key: &str| -> Option<u8> {
        let idx = text.find(key)?;
        text[idx + key.len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    };
    match (find_after("Type"), find_after("Rev")) {
        (Some(t), Some(r)) => {
            let mut id = from_c4board(None, Some(t), Some(r));
            id.running = Running::Cefdk;
            id
        }
        _ => Identity::none(Running::Cefdk),
    }
}
