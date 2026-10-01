//! CEFDK MFH (Master Flash Header) item-table editing — the piece that makes a
//! *no-serial* openHC install safe.
//!
//! The MFH item table lives at [`TABLE_OFF`] in SPI-NOR (`mtd0`). The CEFDK mask
//! ROM validates it with a plain **SHA-256** over the bytes preceding the hash,
//! so any edit MUST recompute that hash or the box hangs before serial (a real
//! brick, since the recovery button does not restore `mtd0`). This module
//! reproduces exactly what CEFDK's own `mfh add script` does, verified
//! byte-for-byte against a live controller.
//!
//! Layout (all offsets relative to [`TABLE_OFF`]):
//!   0x000  header: [ver=1, 0, next_ptr, count, ver2=1, 0,0,0]  (8×u32)
//!   0x020  items, [`ITEM_LEN`] bytes each:
//!            [flags(u32), offset(u32), size(u32), 0,0,0, type(u32), 0]
//!   0x1e0  32-byte SHA-256 of [0x000 .. 0x1e0]
//!
//! The quirk that bit us: the LAST populated item's `type` field always reads 0
//! (a terminator); an item's real type is only written once a later item
//! supersedes it. So appending an item means writing the *previous* last item's
//! real type as well.

use sha2::{Digest, Sha256};

/// SPI-NOR byte offset of the MFH item table.
pub const TABLE_OFF: usize = 0x80000;
/// Header size, then 32-byte items.
pub const HDR_LEN: usize = 0x20;
pub const ITEM_LEN: usize = 0x20;
/// Offset (within the table) of the count field, and of the SHA-256.
pub const COUNT_OFF: usize = 0x0c;
pub const HASH_OFF: usize = 0x1e0;
/// `flags` marking an item valid.
pub const FLAG_VALID: u32 = 0x8000_0000;
/// The type value CEFDK stores for a `script` item (also what any last item
/// reads, being the terminator).
pub const TYPE_SCRIPT: u32 = 0;
/// The real type of the `kernel` item — the last item on the EA family, whose
/// real type must be written when a script is appended after it.
pub const TYPE_KERNEL: u32 = 0x15;

/// Where CEFDK places new user-managed item content on the EA family: just past
/// `ip_params` (0x91010), rounded up to a 0x200 boundary.
pub const EA_CONTENT_OFF: u32 = 0x91200;

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Append a `script` item to the MFH table region `tbl` (exactly the 0x200 bytes
/// starting at [`TABLE_OFF`]), pointing at `content_off`/`content_len`, and
/// recompute the SHA-256. `prev_last_type` is the real type of the item that is
/// currently last ([`TYPE_KERNEL`] on the EA family). Returns the item index
/// used, or an error if the table is full or already has a script.
///
/// This mirrors CEFDK `mfh add script` byte-for-byte (see the module docs).
pub fn append_script(
    tbl: &mut [u8],
    content_off: u32,
    content_len: u32,
    prev_last_type: u32,
) -> Result<usize, String> {
    if tbl.len() < 0x200 {
        return Err("table region must be at least 0x200 bytes".into());
    }
    let count = rd32(tbl, COUNT_OFF) as usize;
    if count == 0 {
        return Err("empty MFH table".into());
    }
    // New item goes at slot `count`; its entry must stay before the hash.
    let new_item = HDR_LEN + count * ITEM_LEN;
    if new_item + ITEM_LEN > HASH_OFF {
        return Err("MFH table full — no room for another item before the hash".into());
    }
    // The current last item (slot count-1) reads type 0; write its real type.
    let last = HDR_LEN + (count - 1) * ITEM_LEN;
    wr32(tbl, last + 0x18, prev_last_type);
    // Write the new script entry (its type field stays 0 as the new terminator).
    for b in &mut tbl[new_item..new_item + ITEM_LEN] {
        *b = 0;
    }
    wr32(tbl, new_item + 0x00, FLAG_VALID);
    wr32(tbl, new_item + 0x04, content_off);
    wr32(tbl, new_item + 0x08, content_len);
    wr32(tbl, new_item + 0x18, TYPE_SCRIPT);
    // Bump the count and recompute the integrity hash.
    wr32(tbl, COUNT_OFF, (count + 1) as u32);
    let digest = Sha256::digest(&tbl[0..HASH_OFF]);
    tbl[HASH_OFF..HASH_OFF + 32].copy_from_slice(&digest);
    Ok(count)
}

/// Remove the trailing `script` item this module's [`append_script`] added,
/// returning the table to stock: zero the script entry (the last item), clear the
/// now-last item's real type back to the terminator 0, decrement the count and
/// recompute the SHA-256. The exact inverse of `append_script`, so
/// `remove_script(append_script(clean)) == clean` byte-for-byte — which is what
/// makes a software return-to-stock safe: CEFDK then boots the stock kernel item
/// again, exactly as it did before openHC was installed.
///
/// Refuses unless the last item actually looks like our script (valid flag, type
/// 0 terminator, pointing into the EA user-content region) so it can never strip
/// a legitimate stock item. Returns the new count.
pub fn remove_script(tbl: &mut [u8]) -> Result<usize, String> {
    if tbl.len() < 0x200 {
        return Err("table region must be at least 0x200 bytes".into());
    }
    let count = rd32(tbl, COUNT_OFF) as usize;
    if count < 2 {
        return Err("MFH table has no appended item to remove".into());
    }
    let script = HDR_LEN + (count - 1) * ITEM_LEN;
    // Guard: only ever remove OUR script entry, never a stock item. Ours is a
    // valid, type-0 item pointing into the EA user-content window (0x90000..
    // 0xa0000, where append_script places it) and is small — the stock kernel
    // item also reads type 0 as the terminator, but lives far outside that window
    // and is megabytes long.
    let sflags = rd32(tbl, script + 0x00);
    let soff = rd32(tbl, script + 0x04);
    let ssize = rd32(tbl, script + 0x08);
    let stype = rd32(tbl, script + 0x18);
    if sflags != FLAG_VALID
        || stype != TYPE_SCRIPT
        || !(0x9_0000..0xa_0000).contains(&soff)
        || ssize >= 0x1000
    {
        return Err("last MFH item is not an openHC script entry — refusing".into());
    }
    for b in &mut tbl[script..script + ITEM_LEN] {
        *b = 0;
    }
    // The item that is now last becomes the terminator: its type reads 0 again.
    let last = HDR_LEN + (count - 2) * ITEM_LEN;
    wr32(tbl, last + 0x18, 0);
    wr32(tbl, COUNT_OFF, (count - 1) as u32);
    let digest = Sha256::digest(&tbl[0..HASH_OFF]);
    tbl[HASH_OFF..HASH_OFF + 32].copy_from_slice(&digest);
    Ok(count - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The clean 12-item MFH table read from a live EA1 (hash 91aaef98...).
    const CLEAN_TABLE: &str = "0100000000000000000208000c0000000100000000000000000000000000000000000080000000000000010000000000000000000000000001000000000000000000008000f00700000800000000000000000000000000000900000000000000000000800008090008070000000000000000000000000000ffffffff00000000000000800000e000000000000000000000000000000000000200000000000000000000800000010000f0060000000000000000000000000003000000000000000000008000f807000008000000000000000000000000000010000000000000000000008000080800000001000000000000000000000000001200000000000000000000800010090010000000000000000000000000000000040000000000000000000080080f0a00f0050000000000000000000000000000080000000000000000000080f8140a0038d20f0000000000000000000000000005000000000000000000008000e819000000010000000000000000000000000007000000000000000000008000e81a00c0f36a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000098efaa915dcf35e78d8c3085e2e96c5321c3e7fdcca53d324b7d43994d123e51";

    fn clean() -> Vec<u8> {
        (0..CLEAN_TABLE.len() / 2)
            .map(|i| u8::from_str_radix(&CLEAN_TABLE[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn clean_table_hash_is_sha256_of_the_head() {
        let t = clean();
        assert_eq!(&Sha256::digest(&t[0..HASH_OFF])[..], &t[HASH_OFF..HASH_OFF + 32]);
    }

    #[test]
    fn append_script_reproduces_cefdk_mfh_add_exactly() {
        // CEFDK `mfh add script 0 <src> 0x100` on this unit produced a table
        // whose SHA-256 is this (captured live). Reproduce it.
        let mut t = clean();
        let idx = append_script(&mut t, EA_CONTENT_OFF, 0x100, TYPE_KERNEL).unwrap();
        assert_eq!(idx, 12, "script appended as the 13th item");
        assert_eq!(rd32(&t, COUNT_OFF), 13);
        let expected = "b817c784c83f718da0d536f452769cc607d1c79e35085f2b3792b06779062c07";
        let got: String = t[HASH_OFF..HASH_OFF + 32].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, expected, "table hash must match CEFDK's own output");
        // The previous last item (kernel) now carries its real type.
        assert_eq!(rd32(&t, HDR_LEN + 11 * ITEM_LEN + 0x18), TYPE_KERNEL);
        // The new script entry.
        assert_eq!(rd32(&t, HDR_LEN + 12 * ITEM_LEN + 0x00), FLAG_VALID);
        assert_eq!(rd32(&t, HDR_LEN + 12 * ITEM_LEN + 0x04), EA_CONTENT_OFF);
        assert_eq!(rd32(&t, HDR_LEN + 12 * ITEM_LEN + 0x08), 0x100);
    }

    #[test]
    fn hash_stays_self_consistent_for_any_size() {
        let mut t = clean();
        append_script(&mut t, EA_CONTENT_OFF, 241, TYPE_KERNEL).unwrap();
        assert_eq!(&Sha256::digest(&t[0..HASH_OFF])[..], &t[HASH_OFF..HASH_OFF + 32]);
    }

    #[test]
    fn remove_script_is_the_exact_inverse_of_append() {
        // append then remove must reproduce the clean table byte-for-byte,
        // including the integrity hash — the guarantee return-to-stock relies on.
        let orig = clean();
        let mut t = orig.clone();
        append_script(&mut t, EA_CONTENT_OFF, 0x100, TYPE_KERNEL).unwrap();
        assert_ne!(t, orig, "append must change the table");
        let n = remove_script(&mut t).unwrap();
        assert_eq!(n, 12);
        assert_eq!(t, orig, "remove_script must restore the clean table exactly");
    }

    #[test]
    fn remove_script_refuses_a_clean_table() {
        // A table with no appended script (last item is the stock kernel) must be
        // left untouched — never strip a legitimate stock item.
        let mut t = clean();
        assert!(remove_script(&mut t).is_err());
    }
}
