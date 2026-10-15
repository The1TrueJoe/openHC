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

## The one hard part: reimaging p1 while openHC runs from p1

In this model openHC runs **from** p1, so p1 cannot be reformatted in place — the
reimage must run from an environment that is not rooted on p1. The EA3's stock
`recovery_kernel.deb` is **not** the tool for this: its `./tmp/bzImage` is a
7,009,216-byte Control4 **container** (== the stock kernel container size), not a
Linux bzImage — `kexec` refuses it, and the deb's real recovery is CEFDK/script
driven (`factory_restore.sh` / `init_kernel` / `kernel_size_writer`), not a Linux
kexec. (`recovery_reimage_p1` now extracts correctly — `gzip -dc` then `tar -xf`,
busybox `tar -a` can't autodetect gzip — and `classify_payload` detects the
container and refuses rather than pretending to boot it.)

So we do it with **openHC's own machinery**, which is already proven to RAM-boot:

1. `ohc-restore stock` boots openHC into a **RAM-only** environment (not pivoting
   to p1) — either by kexec-ing the openHC kernel with a recovery initramfs, or by
   having `boot-init` honour a one-shot `ohc.recovery` flag and stay in RAM instead
   of pivoting. (The durable autoscript already stages kernel + boot-init in RAM
   before the pivot, so the RAM stage exists.)
2. In RAM (p1 is now free): `mount -o ro /dev/mmcblk0p2`, recreate the stock p1
   filesystem, and `xz -dc recfs.tar.xz | tar -x` onto p1 — replicating whatever
   `factory_restore.sh` does beyond the tar (partition flags, kernel-container
   re-lay if any, psvn/serial handling).
3. Undo openHC's SPI-NOR change (`revert`: remove the MFH item / clear the enable
   byte), optionally zero the gap kernels.
4. Reboot → enable byte is off → CEFDK boots the stock kernel item → stock on p1.
   Pristine.

Dependencies to confirm on the box (non-destructive reads):
- openHC rootfs carries `xz`/`unxz` + `tar` (`which unxz xz; busybox | grep unxz`).
- the exact `factory_restore.sh` contract:
  ```
  mount -o ro /dev/mmcblk0p2 /mnt/ohc-recovery
  mkdir -p /tmp/rk && cd /tmp/rk
  ar x /mnt/ohc-recovery/recovery_kernel.deb && gzip -dc data.tar.gz | tar -xf -
  cat etc/factory_restore.sh etc/init.d/init_kernel ; file tmp/kernel_size_writer
  ls -la /mnt/ohc-recovery        # confirm recfs.tar.xz name/size
  ```

## The switch surface (ohc-restore → iod → web UI / MQTT)

- **`ohc-restore status`** — report current boot target (enable byte) + whether a
  stock restore is possible (p2 `recfs.tar.xz` present). (Done, extend wording.)
- **`ohc-restore stock`** — the one-way return to factory above (RAM-boot →
  reimage p1 from p2 → revert SPI-NOR → reboot). Destructive, confirm-gated. This
  is the `restore.stock` iod already exposes; the web UI already has the red
  two-step "Return to stock" control — its backend becomes the real reimage.
- **stock → openHC** is the installer (`ohc-flash`), not `ohc-restore`; the box is
  stock Control4 at that point, with no openHC services to drive it.

No `switch`/coexistence commands, no new partitions, no loopback image — those
were for the instant-toggle model we rejected in favour of this one.

## Status

Landed (PR #43, no box needed): the tar-extraction fix and the
container-classification guard so `stock` can never strand the box. Remaining,
pending the box reads above and live validation: the RAM-boot reimage of p1 from
`recfs.tar.xz` (step 1–3), wiring it behind `ohc-restore stock`, and confirming
the factory_restore.sh contract so the reimage is faithful.
