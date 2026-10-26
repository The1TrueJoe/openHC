# EA remote stock ⇄ openHC switching

How an EA controller (Intel CE5300: EA1 / EA3 / EA5) is switched between **stock
Control4** and **openHC** remotely, both directions, with no physical button.

Consolidates the investigation from the closed PR #43 (the model, the eMMC map,
the `do_ninjago` finding) with the mechanism actually shipped here on #66.

## Model: asymmetric, the way Control4 itself works

Control4's factory restore does **not** roll back the running system — it **wipes
p1 and lays down whatever image is stashed in the recovery partition** (p2),
independent of what is currently on p1. That is why an OS 3.x controller restores
to factory: the restore source (p2) never changed, only p1 did. We use that
asymmetry:

- **openHC → stock is native.** The stock rootfs is already in p2
  (`recfs.tar.xz`, `VERSION=3.3.0.628678-res`). "Switch to stock" *is* a factory
  restore: put p2's rootfs back on p1 and undo openHC's one SPI-NOR change. The box
  comes back as if it never knew openHC.
- **stock → openHC is the network install.** Nothing stock-side carries openHC, so
  the way back is the existing takeover (`ohc-flash`): the SSH/CEFDK-autoscript
  install that put openHC on the box in the first place. No new on-box machinery.

One rootfs on p1 at a time; each direction reuses a mechanism that already exists.

## What "back to stock" undoes

| openHC changed | where | undo |
|---|---|---|
| p1 rootfs | eMMC p1 | reimage from p2 `recfs.tar.xz` |
| MFH autoscript item | SPI-NOR `0x80000` table | leave disabled (do not delete — brick risk) |
| script-enable byte | SPI-NOR `0xa0f1c` | clear (`revert`) |
| openHC kernel + boot-init | eMMC gap `0x800000`/`0x1000000` | (optional) zero |

openHC never touches the stock kernel container at eMMC `0x400` (it boots its own
kernel from the gap), so the stock kernel is already pristine, and p2 is never
written. "Functionally pristine" = enable byte off + p1 = stock rootfs → CEFDK
boots the stock kernel item exactly as from the factory.

## The boot selector (proven, PR #40)

CEFDK chooses what to boot from the MFH table at `0x80000` plus the **enable byte**
at `0xa0f1c`:

| enable byte | CEFDK boots | root |
|---|---|---|
| `0x00` (on) | openHC autoscript → kernel+boot-init from the eMMC gap → pivot | openHC on p1 |
| `0x01`/`0xff` (off) | stock kernel item (container @ `0x400`) | `/dev/mmcblk0p1` (stock) |

Flipping that byte over `/dev/mtd0` is remote, reversible and verified
(`restore revert`/`install`, live on the EA3). It is a 64 KiB
read-modify-erase-write with read-back verify; a bad MFH *table* write bricks past
the recessed button, so the only writes we do are the byte-exact, unit-tested
enable-byte toggle — never a hand-rolled table append on a live box.

## eMMC layout

```
0x000      MBR
0x400      stock kernel container (0x580 header, then bzImage) — UNTOUCHED by openHC
0x800000   openHC kernel    (gap)
0x1000000  openHC boot-init (gap)
0x2000000  p1 (32 MiB in).  p1 = stock rootfs ~513 MB, ext4 LABEL=rootfs
           p2 = factory recovery — SACRED (recfs.tar.xz lives here)
           p3 = 32 MB
```

## The factory mechanism, and why we don't invoke it directly

p2's `factory_restore.sh` maps the EA family to `do_ninjago()`, whose whole body is:

```sh
do_ninjago() {   # ea1 | ea3 | ea5 | amp1
    /usr/dtsbin/flash mfhdelete -type spinor -mfh c4_params -id 0
    echo -n -e "\x01\x00\x00\x00" > /tmp/c4_params.bin
    /usr/dtsbin/flash mfhadd   -type spinor -mfh c4_params -id 0 -file /tmp/c4_params.bin
}   # then reboot -f
```

i.e. **set an MFH `c4_params` item to `01 00 00 00`, reboot** — CEFDK then runs its
own factory restore (reimages p1 from `recfs.tar.xz`) and clears the flag. This is
what the recessed ID button does, in firmware. `recovery_kernel.deb` is a
**kernel-install** package (it dd's the stock kernel container to `0x400`), *not* a
rootfs tool — which is why the old "kexec the recovery deb" idea was wrong.

Replaying `do_ninjago` ourselves means a **new MFH-table item append + SHA-256
rehash on a live, already-openHC-modified table** — an unvalidated extrapolation of
`mfh::append_script`, and a wrong table write bricks past the button. So we do not
do it blind. Instead we reproduce the *effect* using only proven SPI-NOR ops.

## Shipped mechanism — userspace reimage, no new MFH write, no kexec

The restore has one hard constraint: **p1 cannot be reformatted/overwritten while
it is the running root.** We need a moment where p1 is not root. The two obvious
ways to get there both fail or are unavailable on the EA3:

- **kexec a recovery initramfs from durable** — *dead on the EA3.* The installed EA
  boots a "fat" real kernel whose RAM-reclaim `memmap=` leaves no hole for kexec to
  place its segments (`locate_hole failed`, every `-s`/`--mem-min`/`--mem-max`
  variant). Confirmed on hardware.
- **a dedicated RAM-recovery boot mode** — there is no `ohc.ramboot` flag in
  boot-init; the only p1-free RAM path (`net_serve`) deliberately never mounts p1.

So we use the one p1-free context that a *cold reboot* always reaches: **boot-init,
before it pivots.** The autoscript loads the kernel + this ~1–2 MB initramfs into
RAM and runs `/init`, which normally mounts p1 and `switch_root`s into it. At that
instant p1 is only a mount — exactly the context the durable system lacks.

Flow (`restore stock`):

1. **On durable** (`p1_is_root()` true): can't touch p1. Drop a one-shot flag
   `/.ohc-stock-pending` on p1, leave the autoscript enabled, and reboot.
2. **boot-init**, pre-pivot, sees the flag: remove it (so a failed finalize can't
   loop), unmount p1, wait for `/dev/mtd0` (SPI-NOR probes async — MTD is built
   into the same kernel, so the node *does* appear here), then run
   `restore reimage-finalize`:
   - mount p2 ro; **empty p1's existing ext4 in place** and
     `unxz -c recfs.tar.xz | tar -x` onto it. Reusing the ext4 keeps its
     type/LABEL/UUID and needs no `mke2fs` — so no e2fsprogs in the initramfs, only
     busybox `tar`/`unxz`.
   - *only after* a good extract, `revert` (clear the `0xa0f1c` enable byte — the
     one and only SPI-NOR write, byte-exact and unit-tested). The lingering,
     disabled CEFDK script item is left alone (deleting it is the brick risk).
   - `reboot -f`.
3. **Cold boot** → CEFDK (autoscript off) → stock kernel item → stock p1.

When `restore stock` is run somewhere p1 is already free (a flasher ramboot,
whose separate full-rootfs initramfs leaves p1 unmounted), step 1's detour is
skipped and the finalize runs immediately.

**Fail-safe:** if the extract fails, the flag is already gone and the autoscript is
still enabled, so the next boot re-runs boot-init, finds no valid p1, and either
reinstalls openHC from the eMMC-staged image or comes up on the network — the box
heals rather than stranding. p2 is never written in any path.

## Code

- `restore stock` — `cmd_stock`: `p1_is_root()` → `stage_stock_flag()` (durable)
  or `cmd_reimage_finalize()` (p1 free).
- `restore reimage-finalize` — `cmd_reimage_finalize` = `reimage_p1_direct()` +
  `revert`. Called by boot-init; refuses if p1 is root.
- `restore recovery-kexec` — the old recovery-kernel kexec, kept for boards
  where kexec can place segments (EA1); not used on the EA3.
- boot-init hook: `board/ea/common/boot-init/init` (flag check + `/dev/mtd0` wait +
  finalize, before `switch_root`).
- initramfs bundling: `build/mk-boot-init.sh` copies the static `restore` and
  symlinks the `tar`/`unxz`/`rm`/`reboot` applets; `board/common/busybox-openhc.fragment`
  pins those applets on.

## The switch surface

- **`restore status`** — boot target (enable byte) + whether a stock restore is
  possible (p2 `recfs.tar.xz` present).
- **`restore stock`** — one-way return to factory via the flow above.
  Destructive; the red two-step "Return to stock" control in the web UI (via iod's
  `restore.stock`) is its front end.
- **stock → openHC** — the installer (`ohc-flash`); at that point the box is stock
  Control4 with no openHC services to drive it.

## Remaining: one destructive hardware test

Not yet run end-to-end on the EA3 (the box is recoverable here via serial + p2 + a
CH341A). On durable openHC: `restore stock`, reboot, and watch serial for the
boot-init flag path — expect the p1 empty+extract, the `revert`, the reboot, and
stock Control4 back on p1 — then reinstall openHC with `ohc-flash` to return.
