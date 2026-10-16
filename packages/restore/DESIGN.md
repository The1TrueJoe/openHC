# EA remote stock ⇄ openHC switching

This is the design for switching an EA controller (Intel CE5300: EA1 / EA3 / EA5)
between **stock Control4** and **openHC** remotely, both directions, with no
physical button press.

## Model: asymmetric, the way Control4 itself works

Control4's factory restore does **not** roll back the running system — it **wipes
p1 and lays down whatever image is stashed in the recovery partition**, which is
independent of whatever is currently on p1. That is why a controller running
OS 3.x restores to the factory 2.10: the restore source (p2) never changed, only
p1 did. We use exactly that asymmetry:

- **openHC → stock is free and native.** The stock rootfs is already stashed in
  p2 (`recfs.tar.xz`, OS 3.3.0, `VERSION=3.3.0.628678-res`). "Switch to stock" IS
  a factory restore: reimage p1 from p2 and undo openHC's SPI-NOR change. The box
  comes back **as if it never knew openHC existed.**
- **stock → openHC is the network install.** Nothing stock-side carries openHC, so
  the way back is the existing takeover (`ohc-flash`): the SSH/CEFDK autoscript
  install that put openHC on the box in the first place. No new on-box machinery.

This is deliberately simpler than keeping both OSes resident (no coexistence
partition, no loopback rootfs, no A/B scheme): one rootfs on p1 at a time, and
each direction reuses a mechanism that already exists.

## What "back to stock" means concretely (pristine)

openHC makes a small, bounded set of changes to a factory EA. Returning to stock
undoes exactly these:

| openHC changed | where | undo |
|---|---|---|
| p1 rootfs (dd'd openHC over it) | eMMC p1 | reimage from p2 `recfs.tar.xz` |
| MFH autoscript item | SPI-NOR `0x80000` table | remove it (openHC-appended) / leave disabled (CEFDK-created) |
| script-enable byte | SPI-NOR `0xa0f1c` | clear (`script off`) |
| openHC kernel + boot-init | eMMC gap `0x800000`, `0x1000000` | (optional) zero the gap |

Notably openHC does **not** touch the stock kernel container at eMMC `0x400` (it
boots its own kernel from the gap via `emmc rd_up`), so the stock kernel is
already pristine. And p2 (factory recovery) is never written — sacred.

Two grades of "pristine":
- **Functionally pristine** (what we do): enable byte off + MFH item removed (or
  left disabled) + p1 = stock rootfs → CEFDK boots the stock kernel item exactly
  as from the factory. Reversible, no extra SPI-NOR risk.
- **Byte-pristine** (optional, extra risk): physically delete a CEFDK-created MFH
  script item (another MFH-table write → brick-past-button risk) and zero the gap
  kernels. Only if a truly byte-identical factory image is required; otherwise the
  disabled item is inert and the gap is unread.

## The boot selector (already proven — PR #40)

CEFDK chooses what to boot from the MFH table at SPI-NOR `0x80000` plus the
**script-enable byte** at `0xa0f1c`:

| enable byte | CEFDK boots | root |
|---|---|---|
| `0x00` (on) | openHC autoscript → kernel+boot-init from the eMMC gap → pivot | openHC on p1 |
| `0x01`/`0xff` (off) | stock kernel item (container @ `0x400`) | `/dev/mmcblk0p1` (stock) |

Flipping that byte over `/dev/mtd0` is remote, reversible, and verified
(`ohc-restore revert`/`install`, validated live on the EA3 2026-10-15). It is a
64 KiB read-modify-erase-write with read-back verify; a bad MFH write bricks past
the recessed button, so every write is the byte-exact inverse of the install's
(`ohc_flash_core::mfh`, unit-tested).

## eMMC layout

```
0x000        MBR (0x55aa @ 0x1fe)
0x200        stock kernel container size, BE u32 (0x6af3c0 = 7,009,216)
0x400        stock kernel container  (0x580 header, then the bzImage) — UNTOUCHED by openHC
0x800000     openHC kernel    (gap)
0x1000000    openHC boot-init (gap)
0x2000000    p1 begins (32 MiB in).  p1 = 6 GB (stock rootfs ~513 MB)
             p2 = 1 GB  factory recovery — SACRED (recfs.tar.xz lives here)
             p3 = 32 MB
```

## The real mechanism (confirmed from p2's `factory_restore.sh`, 2026-10-15)

The EA factory restore is **not** a Linux reimage of p1 at all. Control4's own
`factory_restore.sh` (shipped in p2's `recovery_kernel.deb`) maps the EA family to
`do_ninjago()`, whose entire body is:

```sh
do_ninjago() {   # ea1 | ea3 | ea5 | amp1
    /usr/dtsbin/flash mfhdelete -type spinor -mfh c4_params -id 0
    echo -n -e "\x01\x00\x00\x00" > /tmp/c4_params.bin
    /usr/dtsbin/flash mfhadd   -type spinor -mfh c4_params -id 0 -file /tmp/c4_params.bin
}
# …then main() does `reboot -f`.
```

That is the whole thing: **set an MFH `c4_params` item to `01 00 00 00`, reboot.**
On the next boot CEFDK sees the flag and runs its own factory restore — reimaging
p1 from `recfs.tar.xz` and whatever else the factory operation entails — then
clears the flag. This is exactly what the recessed ID button triggers; the button
and `factory_restore.sh` both just set `c4_params`, one in firmware and one via
Control4's `/usr/dtsbin/flash`. `recovery_kernel.deb` is a **kernel-install**
package (`kernel_size_writer` dd's the stock kernel container to eMMC `0x400`), not
a rootfs tool — which is why our old `kexec`-the-deb approach was wrong, and why
`recovery_reimage_p1` now classifies that container and refuses rather than booting
it.

### The MFH state on the live EA3 (why `c4_params` is the hard part)

`ohc-restore status` dumps 13 items; **none is `c4_params`** — it is not a resident
item, it is *created on demand* to request the restore (the factory tool does
`mfhdelete` (no-op) then `mfhadd`). So setting the flag is an **MFH item append +
SHA-256 rehash**, the same structural op as our autoscript append, and the table
has room for exactly one more item (count 13 → item[13] at `0x801c0`, right before
the hash at `0x801e0`). The freed, DELETED item[2] region (`0x90800`, 0x708 bytes,
in the already-RMW'd `0x90000` block) is clean space for the 4-byte content.

**The catch:** `mfh::append_script` is trusted because its output was captured
byte-for-byte from CEFDK's own `mfh add script` on the **EA1's clean 12-item
table** (the `b817c784…` hash in its tests). Appending `c4_params` on **this EA3's
13-item, already-openHC-modified table** is an *unvalidated extrapolation* — and a
wrong MFH write bricks past the button. So we do not hand-roll it blind.

## Two ways to return to stock — pick one

### Path 1 — CEFDK-native `c4_params` flag (faithful, needs validation)
`ohc-restore stock` writes the `c4_params` MFH item (`01 00 00 00`) + clears
openHC's autoscript (the proven `revert`), then reboots; CEFDK does the real
factory restore. **Before any live write**, validate our computed table against
CEFDK's own tool byte-for-byte, exactly as the autoscript was: capture
`flash mfhadd -mfh c4_params` (over serial, or run the stock `/usr/dtsbin/flash`
extracted from `recfs.tar.xz`) on this box and match the resulting table + hash in
a `mfh.rs` unit test. Pros: faithful to the factory operation (CEFDK does the whole
restore, including anything beyond the rootfs). Cons: a new MFH-table write to
validate; brick-past-button if wrong (mitigated here — this lab box has serial +
p2 + a CH341A SPI programmer, so it is recoverable).

### Path 2 — userspace RAM-reimage (only proven SPI-NOR ops) ← **lower risk**
Reproduce the *effect* without touching `c4_params`:
1. `revert` openHC's autoscript — the enable byte + `remove_script`, both proven
   byte-exact. (CEFDK will then boot the stock kernel item, which openHC never
   touched — it lives at eMMC `0x400`, intact.)
2. Reimage p1 from a RAM environment (openHC can RAM-boot; the autoscript already
   stages kernel+boot-init in RAM before the pivot, so a one-shot `ohc.recovery`
   flag that keeps `boot-init` in RAM gives us a non-p1 root): `mount -o ro
   /dev/mmcblk0p2`, clean p1, `unxz -c recfs.tar.xz | tar -x -C /p1`. openHC has
   `unxz` + `tar` (confirmed on the box).
3. Reboot → stock kernel item → stock rootfs on p1.

Pros: **no new MFH write** — only the already-proven `revert` touches SPI-NOR; the
risky part is a userspace tar extract, fully recoverable. Cons: reproduces the
rootfs restore ourselves rather than invoking CEFDK's, so if the factory restore
does more than lay down `recfs.tar.xz` (unlikely to matter — kernel is untouched,
p2 is intact) we would not replicate it; and it needs a small RAM-recovery mode in
`boot-init`.

**Chosen: Path 2** (no unproven SPI-NOR write). Path 1 remains available later if a
byte-exact factory operation is ever wanted (capture a `flash mfhadd c4_params` diff
first to validate it).

### Path 2 as implemented (`ohc-restore stock --yes`)

1. Preconditions (fail before anything destructive): `kexec`/`unxz`/`tar`/
   `busybox`/`cpio` present; p2 holds `recfs.tar.xz`.
2. Stage the openHC kernel from the eMMC gap (`0x800000`; `/boot` is empty), verify
   its `HdrS` magic.
3. Build a tiny **recovery initramfs** (busybox + its loader/libc + `ohc-restore`
   itself + a shell `/init`) and `kexec -l … --initrd=… --command-line="… ohc.recovery=stock"`.
4. The recovery `/init`, running in RAM (p1 is not its root): mount p2 ro; **reuse
   p1's existing ext4** (empty it — no `mkfs`, so no e2fsprogs closure, and p1 keeps
   its LABEL/UUID); `unxz -c recfs.tar.xz | tar -x -C /mnt/p1`; then — **only after a
   verified-good extract** — `ohc-restore revert` (the proven enable-byte + MFH-item
   clear, the one and only SPI-NOR write); `reboot -f`.
5. Cold boot → CEFDK (autoscript reverted) → stock kernel item → stock p1.

Fail-safe: if the extract fails, the autoscript is left enabled, so the next boot
re-runs openHC's `boot-init`, which reinstalls openHC from the eMMC-staged image —
the box heals back to openHC rather than stranding. kexec is a soft load that
ignores the MFH, so the recovery runs regardless of boot-config state.

**The one destructive live test** (box recoverable here via serial + p2 + CH341A):
cross-build `ohc-restore` (host → i686-musl, run from `/tmp`), then
`ohc-restore stock --yes` and watch serial: expect the kexec into the recovery
initramfs, the p1 empty+extract, `revert`, reboot, and stock Control4 back on p1
(nginx at the stock IP). The on-box `/opt/ohc/bin/ohc-restore` is stale (pre-#40),
so use the freshly built one.

## The switch surface (ohc-restore → iod → web UI / MQTT)

- **`ohc-restore status`** — current boot target (enable byte) + whether a stock
  restore is possible (p2 `recfs.tar.xz` present). (Wording to extend.)
- **`ohc-restore stock`** — the one-way return to factory via the chosen path
  above. Destructive, confirm-gated. This is the `restore.stock` iod already
  exposes; the web UI already has the red two-step "Return to stock" control — its
  backend becomes the real restore.
- **stock → openHC** is the installer (`ohc-flash`), not `ohc-restore`; the box is
  stock Control4 at that point, with no openHC services to drive it.

No `switch`/coexistence commands, no new partitions, no loopback image — those were
for the instant-toggle model we rejected in favour of this one.

## Status

Landed (PR #43): `ohc-restore stock --yes` implements Path 2 end to end (kexec a
recovery initramfs → rebuild p1 from `recfs.tar.xz` → proven `revert` → reboot),
host-compiles with unit tests pinning the recovery `/init` contract; iod's
`restore.stock` passes `--yes`; the web UI's existing red "Return to stock" control
drives it unchanged. Confirmed on the live EA3 (read-only, 2026-10-15): the real
restore mechanism, that `recovery_kernel.deb` is a kernel installer, that openHC
has `unxz`/`tar`/`kexec`, that p1 is ext4 `LABEL=rootfs` and `recfs.tar.xz` is a
plain tar, and that the box's installed `/opt/ohc/bin/ohc-restore` is **stale
(pre-#40)**. Remaining: the one destructive live test above (not run — the box is
mid switch-DDS reflash and owned by that session).
