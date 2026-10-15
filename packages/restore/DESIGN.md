# EA remote stock ⇄ openHC switching

This is the design for switching an EA controller (Intel CE5300: EA1 / EA3 / EA5)
between **stock Control4** and **openHC** entirely in software, both directions,
with **no physical button press** and no on-site presence.

Status: the fast boot-path flip is already proven on hardware; the two pieces that
make it *safe* to flip (coexistence) and the full one-way reimage are designed
here and gated on decisions + a few non-destructive box reads (see the last
section). This file is the spec those follow.

---

## 1. What the box actually does at boot

CEFDK (the signed Intel firmware in SPI-NOR / `mtd0`) decides what to boot from
the **MFH** (Master Flash Header) table at NOR `0x80000` plus one enable byte:

- **Stock**: CEFDK boots the stock **kernel item** (MFH type `0x15`) — the kernel
  container at eMMC `0x400` — which runs the stock kernel with
  `root=/dev/mmcblk0p1`. This is the factory path; openHC never touches the kernel
  item, the container, or its cmdline.
- **openHC**: CEFDK runs an **autoscript** (an MFH `script` item + the
  *script-enable byte* at NOR `0xa0f1c`). The autoscript `emmc rd_up`s openHC's
  kernel and `boot-init` out of the eMMC gap into RAM and `bootlinux`es them;
  `boot-init` then pivots to the openHC rootfs.

The **script-enable byte** (`cefdk::SCRIPT_ENABLE_OFF = 0xa0f1c`, CEFDK's own
`script on`/`script off`) is the single authoritative gate:

| enable byte | CEFDK boots            | root filesystem           |
|-------------|------------------------|---------------------------|
| `0x00` (on) | openHC autoscript      | openHC rootfs             |
| `0x01`/`0xff` (off) | stock kernel item | `/dev/mmcblk0p1` (stock) |

Flipping that one byte over `/dev/mtd0` is **proven, reversible, and remote**
(`ohc-restore revert`/`install`, PR #40, validated live on the EA3 2026-10-15:
the byte toggles and reads back, openHC stays up). So the *boot selector* is
solved. It is a 64 KiB read-modify-erase-write with read-back verify; a bad MFH
write bricks past the recessed button (SPI-NOR recovery needs an external
programmer), which is why every write is verified and the table edit is the
byte-exact inverse of the install's (`ohc_flash_core::mfh`, unit-tested there).

## 2. The problem

The boot *selector* is independent and remote. What is **not** independent is the
**rootfs**: both paths use p1. openHC's install `dd`'d its rootfs over
`/dev/mmcblk0p1`, overwriting the stock rootfs. So today:

> flip the enable byte to "boot stock" → **stock kernel + openHC's p1** → the
> stock kernel mounts an openHC root and the box comes up broken.

For an **instant** software switch (the enable-byte flip alone, no reimage), stock
and openHC must **coexist**: stock rootfs on p1 (factory, untouched) and openHC's
rootfs somewhere else, with openHC's `boot-init` pivoting to that somewhere-else.

Only the **rootfs** conflicts. openHC's kernel + `boot-init` already live in the
eMMC gap (`0x800000`, `0x1000000`), clear of the stock kernel container at
`0x400`. So coexistence is entirely about *where openHC's rootfs lives* and
*what `boot-init` pivots to*.

## 3. eMMC layout (measured; one number to confirm — see §7)

```
0x000        MBR (0x55aa @ 0x1fe), partition table @ 0x1be
0x200        stock kernel container size, BE u32 (0x6af3c0 = 7,009,216)
0x400        stock kernel container  (0x580 header, then the bzImage)
0x800000     openHC kernel   (gap; emmc rd_up target)
0x1000000    openHC boot-init (gap; emmc rd_up target)
0x2000000    p1 begins (sector 65536, 32 MiB in)
             p1 = 6 GB  (stock rootfs ~513 MB used)
             p2 = 1 GB  factory recovery — SACRED, never written
             p3 = 32 MB
             (free space after p3? — the one unknown for option A)
```

eMMC writes are recoverable by the recessed button / CEFDK factory restore
(the button reimages eMMC); only **SPI-NOR / MFH** writes are brick-past-button.
So eMMC partition / rootfs edits carry far less risk than MFH edits.

## 4. Coexistence — options

Keep **stock on p1** (the factory kernel item's `root=p1` is left exactly as
shipped). Move **openHC's rootfs off p1** and point `boot-init` at the new home.

### Option A — dedicated partition `p4`
Carve a new primary partition after p3 for openHC's rootfs.
- `boot-init` pivots to `/dev/mmcblk0p4` (by label, not number, to be safe).
- Needs: free space after p3 (confirm total eMMC size) **and** an MBR
  partition-table edit (add entry 4 at MBR `0x1ee..0x1fe`; p1/p2/p3 entries, the
  `0x55aa`, and the size field at `0x200` all untouched).
- Pros: a first-class filesystem, independent `fsck`, openHC never inside stock's
  tree. Cons: depends on geometry; one partition-table write (eMMC, so
  button-recoverable, but still a write to sector 0).

### Option B — loopback rootfs image on stock p1  ← **recommended**
Store openHC's rootfs as an ext image file on the stock p1, e.g.
`/openhc/root.img`.
- `boot-init` mounts p1, loop-mounts `/openhc/root.img`, pivots to it.
- Needs: **nothing about geometry, no partition-table write.** p1 is 6 GB with
  ~513 MB used — ample room for a few-hundred-MB image. Stock never touches the
  `/openhc` directory; openHC only reads/writes its own image file.
- Pros: zero partition-table risk, zero geometry dependence, trivially reversible
  (`rm /openhc/root.img`), and the openHC rootfs is already produced as an ext
  image that drops straight into a file. Cons: openHC's root is nested in stock's
  p1 filesystem (acceptable — stock won't touch our dir; p1 health is the only
  shared dependency).

### Option C — gap / p3 — rejected
The eMMC gap (~32 MB, already holding kernels) and p3 (32 MB) are far smaller than
the openHC rootfs (≥ ~49 MB compressed). No room.

**Recommendation: B.** It needs no partition-table write and no assumption about
total eMMC size, is the lowest-risk and most reversible, and matches how the
openHC rootfs is already built. A is a clean alternative *if* free space after p3
is confirmed and a (button-recoverable) MBR edit is acceptable.

Either way the two boot paths stay fully distinct and the **enable byte alone
selects them** — stock boots its own kernel item with `root=p1`; openHC's
autoscript boots `boot-init`, which pivots to the openHC home (p4 / loop image),
never to p1. No runtime flag is needed.

## 5. The one-way "reimage p1 back to stock"

Coexistence changes this problem completely. Today `ohc-restore stock` tries to
Linux-`kexec` a "recovery kernel" out of p2's `recovery_kernel.deb`; that is
**wrong for the EA** (confirmed live 2026-10-15):

- `./tmp/bzImage` in that .deb is **7,009,216 bytes = the stock kernel
  *container*** (byte-for-byte the size at eMMC `0x200`), not a bzImage: no
  `0x55aa` at `0x1fe`, a Control4 container header at offset 0. `kexec` refuses it
  ("Not a PE file / can not find .linux section"), even `-t bzImage64`.
- The .deb also ships `./etc/factory_restore.sh`, `./etc/init.d/init_kernel`,
  `./tmp/kernel_size_writer` — i.e. the EA recovery is **CEFDK/script-driven**
  (re-lay the kernel container at `0x400`, set the recovery flag, let CEFDK boot
  recovery and run the scripts), not a plain Linux kexec of a bzImage.

`recovery_reimage_p1` now **extracts correctly** (the busybox `tar -a` does not
autodetect gzip → `gzip -dc` then `tar -xf`), **classifies** the payload
(`classify_payload`: bzImage vs Control4 container), and **refuses without
reverting the boot path** when it is a container — so `stock` can never strand the
box on "stock kernel + openHC p1". If a future recovery image ships a genuine
bzImage, the kexec path still works (revert, then kexec).

**But with coexistence in place, the reimage is far simpler and needs no CEFDK
recovery at all:** openHC runs from p4 / the loop image, so **p1 is no longer
openHC's running root** — openHC userspace can reformat and repopulate p1 directly.
`factory_restore.sh`'s own job is exactly this: unpack p2's `recfs.tar.xz`
(513 MB, stock OS 3.3.0, `VERSION=3.3.0.628678-res`) onto p1. So the software
reimage becomes:

```
mount -o ro /dev/mmcblk0p2 /mnt/ohc-recovery
# (replicating factory_restore.sh faithfully — see §7)
# wipe/mkfs or clean p1, then:
xz -dc /mnt/ohc-recovery/recfs.tar.xz | tar -x -C /mnt/p1
# re-lay the stock kernel container at eMMC 0x400 if the .deb's
# kernel_size_writer / init_kernel do so (confirm from the scripts)
```

No kexec, no CEFDK recovery trigger, no button. The non-coexistence reimage (when
openHC *is* on p1) would still need to boot something else first — a RAM-booted
openHC whose `/init` reimages p1 before pivot, or the real CEFDK recovery trigger —
but **coexistence-first removes that need**, which is the main reason to do
coexistence before the destructive reimage.

Dependencies to confirm: openHC rootfs must carry `xz`/`unxz` (busybox `unxz` or
xz-utils), and the reimage must reproduce whatever `factory_restore.sh` does
beyond the tar (partition flags, the kernel container re-lay, psvn/serial
handling). Hence the ask to read those scripts.

## 6. The switch surface (ohc-restore → iod → web UI / MQTT)

The validated enable-byte flip IS the instant toggle. Reframe and expose it:

- **`ohc-restore switch stock`** — set the enable byte off (keep the MFH script
  item intact, so it is a pure one-byte flip, maximally reversible). Next boot:
  stock. *Only safe once coexistence is installed.*
- **`ohc-restore switch openhc`** — set the enable byte on. Next boot: openHC.
- **`ohc-restore status`** — already reports the enable byte + item; add a
  **`coexist:`** line: whether stock is on p1 *and* an openHC home (p4 / loop
  image) exists. Source of truth: a small marker openHC drops at
  coexistence-install time recording its root location (e.g.
  `/opt/ohc/etc/coexist` = `loop:/openhc/root.img` or `part:/dev/mmcblk0p4`),
  which `boot-init` also consumes as its pivot target.
- **`ohc-restore reimage-stock`** — the destructive, one-way p1 repopulate from
  p2 (§5). Distinct from `switch stock`.

iod (`ops.rs` / `api.rs` / `mqtt`):
- Keep `restore.status` / `/api/system/restore`; extend its JSON with
  `coexist: bool` and `boot: "stock"|"openhc"`.
- Add `restore.switch {target: "stock"|"openhc"}` →
  `/api/system/switch` + `cmd/restore/switch`. **Offered only when
  `coexist == true`** — on a non-coexistence box the enable-byte flip is unsafe,
  so iod reports the switch as unavailable and only the destructive
  `restore.stock`/`reimage-stock` remains.
- Keep `restore.stock` (confirm-gated) as the destructive reimage.

Web UI (`packages/webd/ui/src/panels/System.tsx`):
- When `coexist`, show a plain, **non-destructive** toggle: "Running: openHC
  ⇄ switch to stock" (and back), one click + reboot, no red-button confirm —
  because it is reversible and wipes nothing.
- Keep the existing red two-step "Return to stock" as the destructive reimage,
  shown always (it is the way *in* to coexistence from a plain openHC box, and the
  way to a true factory state).

## 7. Open decisions + the box reads needed (non-destructive)

The live EA3 (10.0.0.200) is owned by the switch-DDS session right now. These are
**read-only**; please run them (or hand them over) rather than both sessions
touching the box:

1. **Coexistence scheme — your call:** Option B (loop image on p1, recommended) or
   Option A (new p4). Picks the install + `boot-init` pivot design.
2. **Partition geometry** (decides A's feasibility, confirms B's headroom):
   `cat /proc/partitions`, `fdisk -l /dev/mmcblk0` (or `sfdisk -d`), `df -h`, and
   what **p3** actually holds (`mount -o ro /dev/mmcblk0p3 /mnt/p3 && ls -la
   /mnt/p3 && umount /mnt/p3`).
3. **The real recovery contract** — the three scripts in p2's recovery deb:
   ```
   mount -o ro /dev/mmcblk0p2 /mnt/ohc-recovery
   mkdir -p /tmp/rk && cd /tmp/rk
   ar x /mnt/ohc-recovery/recovery_kernel.deb
   gzip -dc data.tar.gz | tar -xf -
   cat etc/factory_restore.sh etc/init.d/init_kernel ; file tmp/kernel_size_writer
   ```
   plus `ls -la /mnt/ohc-recovery` (confirm `recfs.tar.xz` size/name) and whether
   the openHC rootfs has `unxz`/`xz` (`which unxz xz; busybox | grep -o unxz`).

With (1) chosen and (2)/(3) in hand, the remaining implementation is: the
coexistence install (restore stock → p1, create the openHC home, drop the marker),
the `boot-init` pivot change, `ohc-restore switch` + `reimage-stock`, and the iod /
web UI wiring above.
