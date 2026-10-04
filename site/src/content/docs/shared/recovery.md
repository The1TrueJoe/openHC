---
title: Recovery
description: Read this before writing anything to a device — what each button does, and the one region on each board that must never be touched.
sidebar:
  order: 4
---

:::danger[Read this before writing anything to a device]
Every board here has a documented path back to stock. Every one of those paths
depends on **not** having written to one specific region. This page is which
region, per board.
:::

## The headline, per board

| Board | The safety net | Never write |
|---|---|---|
| **EA family** | CEFDK lives in **SPI-NOR, separate from the eMMC**, and the recovery button reimages kernel + rootfs + CEFDK from p2 | **p2** — it holds the entire recovery payload |
| **CA-1** | the factory-restore button boots a recovery kernel from **SPI-NOR**, pre-`bootcmd`, needing no password | **p3** (`recfs`) and the SPI-NOR |
| **HC-800** | two vendor GRUB entries and a factory-restore partition, untouched by our install | **sda2** and the vendor's two `menu.lst` entries |
| **IOX v1** | **dual flash banks plus a recovery image**, untouched; openHC lives in the half of the NAND stock never partitioned, and three failed openHC boots fall back to stock automatically | the recovery bank |

## `ohc-flash restore`

One command to put a controller back the way it shipped:

```sh
ohc-flash restore <host>
```

- **IO Extender:** sets U-Boot's compiled-in `bootcmd`, deletes every variable
  openHC added, and erases the openHC slot. Verified on a unit: it comes back with
  the factory environment and boots stock.
- **HC-800:** runs Control4's own factory-restore system (entry 0's kernel line
  on `/dev/sda2`) exactly once, the software equivalent of the ID button. It
  appends a copy of entry 0 that `savedefault`s back to the stock entry and sets
  `default saved`. It never writes `default 0`: `restore.sh` reboots without
  touching `menu.lst`, so that would restore forever. Verified on a unit 2026-10-02.
  An openHC built with the `restore` feature does this itself (`ohc-restore stock`,
  the same rewrite from the same code); stock Control4 gets it over SSH.
- **EA family:** runs the box's `ohc-restore stock`, which removes openHC's single
  MFH item from SPI-NOR (read-back verified) and kexecs p2's recovery kernel to
  re-image p1, the same reimage the recessed button starts.
- **CA-1:** deletes openHC's `boot.scr`, zImage and DTB from the FAT partition and
  makes the next boot run U-Boot's own `factoryrestore` once, which re-images p2
  from p3. The one-shot is plain `setenv` words, no nested quoting: the original
  `bootcmd` is held in `ohc_stock_bootcmd`, and running `restore` again on the
  restored stock box folds it back and removes the helper variables.

`factory-restore` is an alias for the same command.

## EA family

### What the factory-restore button actually does

Measured by capturing the full recovery-kernel log on hardware. The recessed
button is not a light touch, it's a **complete stock reimage sourced entirely
from p2**, in this order:

1. `mke2fs` — **reformats p1**. (An earlier note here claimed it did not run mkfs;
   that was wrong.)
2. mounts p1 and p2, copies the stock rootfs and config from **p2 → p1**.
3. unpacks `recovery_kernel.deb` and `dd`s the **stock 6.7 MB kernel** back over
   the eMMC kernel region — logging `Wrote Kernel Size To Flash Success!`.
4. unpacks `cefdk.deb` and `dd`s **stock CEFDK (512 KB)** back — it even reflashes
   the bootloader.
5. reboots.

**The consequence for going persistent is good:** the button restores kernel,
rootfs *and* CEFDK to stock, so flashing our own kernel and rootfs is fully
reversible with one press.

**The single region that must never be written is p2.** It holds
`recovery_kernel.deb`, `cefdk.deb` and the stock rootfs, and it's the one source
of truth for the reimage.

The restore is **proven on both an EA1 and an EA3**. On the EA3 it ran from the
button, extracted the factory rootfs over p1, and rebooted into a working stock
image in about a minute. There's a benign `device_shutdown` warning in the reboot
path; the machine restarts anyway.

### openHC's own software return-to-stock

The native button above is Control4's. openHC adds a second path that needs no
serial and, with one of the triggers below, no button either. It exists because
the boot SPI-NOR is now a writable `/dev/mtd0` — the in-tree `spi-ea-ce5xx`
controller driver drives the CE5300's dedicated boot-flash block (PCI
`8086:08a0`, a 16 MB S25FL127S), confirmed on an EA1 where `/proc/mtd` shows a
16 MB `mtd0`. See [the boot chain](/ea/boot-chain/).

**openHC's install makes exactly one boot change.** It appends a single `script`
item to the CEFDK Master Flash Header (MFH) item-table in SPI-NOR — the table at
offset `0x80000`, SHA-256 protected — redirecting CEFDK to openHC's autoscript.
The stock kernel item, the stock kernel, and p2's entire factory payload are left
untouched. So "return to stock" is just removing that one item. The tool is
`ohc-restore` (the `restore` feature; this is its EA backend, the HC-800's is
[below](#hc-800)); its MFH edit is
the byte-for-byte inverse of the installer's append, unit-tested to reproduce the
known-stock SHA against the real 16 MB backup. Its subcommands:

| Subcommand | What it does | Writes `mtd0`? |
|---|---|---|
| `status` | parse and print the MFH table | no |
| `revert` | remove openHC's item (stock boot); reversible with `install` while p1 is still openHC | yes — erase + write + read-back verify |
| `install` | re-append openHC's item (undo `revert`) | yes |
| `stock` | `revert`, then recovery-kexec p2's kernel to reimage p1 — the full return to Control4 | yes |
| `restore-mfh <backup.bin> [--compensate]` | rewrite the MFH block from a full-flash backup (repair a corrupted MFH) | yes |
| `erasetest` | erase the MFH block and report how much read back `0xff` (diagnostic) | erase only |

Every write is read-modify-**erase**-write then **read-back-verified**; nothing
here ever claims success without the flash reading back exactly what was intended.

**Three ways to trigger it:**

1. **iod**, over REST (`/api/system/restore` and `/api/system/restore/stock`) and
   MQTT (`cmd/restore/status`, `cmd/restore/stock`). `stock` requires an explicit
   `{confirm:true}`.
2. **The web UI System panel**, a two-step "Reset to stock" confirm, shown only
   when `status.available`.
3. **The front ID button.** Holding it runs `ohc-restore stock`. The button is a
   bare SoC GPIO (`gpiochip0` line 32, active-low — released reads `1`, pressed
   `0`, confirmed on an EA1) that `gpio-ea-board` deliberately leaves unclaimed. A
   watcher (`/opt/ohc/bin/ohc-restore-button`, polling with libgpiod, launched by
   init `S96ohc-restore-button`) fires after `OHC_RESTORE_BUTTON_HOLD` seconds
   (default 10). It is **fail-safe**: it arms only after one clean "released"
   reading, so a wrong line or polarity stays inert rather than wiping a box at
   boot — verified on hardware. Each board configures it in `board.env`
   (`OHC_RESTORE_BUTTON="gpiochip0 32"`, etc.).

:::caution[Why the software path is the reliable one on a fuse-blown EA1]
The native recessed CEFDK button does **not** rewrite the SPI-NOR MFH on a
fuse-blown EA1, so the native button alone won't undo openHC's one change. The
software path is the dependable way back there. On the EA3 the button's GPIO line
and polarity are not yet hardware-confirmed (the `board.env` notes this); the
fail-safe keeps the watcher inert until they are.
:::

:::note[Proven, and not-yet-proven]
The `mtd0` **write path is proven**: it rewrote the MFH on a corrupted live EA1
and read it back verified (the recovery was possible because `kexec` is a soft
load that never reads the MFH, so a fixed kernel could be kexec'd in to repair the
flash). The **full `stock` round-trip** — `revert` the MFH *and* recovery-kexec p2
to reimage p1, all the way back to Control4 — has **not** yet been run end to end
on hardware.
:::

### Three boot modes, chosen at power-on

```
Executing Control4 Normal Boot Mode      → kernel from eMMC, root=/dev/mmcblk0p1
Execute Control4 Recovery Kernel         → factory restore
Entering Control4 Manufacturing Mode     → TFTP-netboots a kernel  ← our dev path
```

If button-GPIO setup fails, CEFDK logs it and falls through to normal boot.

### The recovery matrix

| What you broke | Recoverable? | How | Needs |
|---|---|---|---|
| eMMC rootfs (p1) | **yes, easily** | restore button, or CEFDK shell → `emmc wr` | button, or serial |
| eMMC entirely | **yes** | CEFDK shell → `emmc wr`, or TFTP a rescue kernel | serial |
| Per-unit config on p2 | **yes** | regenerable from the eth0 MAC | ssh |
| CEFDK in NOR | **yes** | YMODEM a working CEFDK — *"Please send a working CEFDK via YMODEM now..."* | serial |
| NOR **and** no serial | **no** | external SPI programmer, clipped onto the flash | hardware |

**Serial console access is the gating requirement for all of it.** Without serial
you have no CEFDK shell, and the safety net is theoretical.

CEFDK also supports **multiple CEFDK slots** in the boot partition, so even a
corrupted bootloader is recoverable with serial access.

### The asymmetry that governs the order of work

- A broken **rootfs** is recoverable over serial — the initramfs `c4` shell still
  runs.
- A broken **kernel** is **not**, because the initramfs providing that escape is
  part of the kernel image that failed to load.

Which is why netboot, which writes nothing, is the right way to prove custom
code, and why the ordering is always: **prove it in RAM, then write it.**

### Backups, before the first write

| Item | Size | Note |
|---|---|---|
| SPI NOR (`/dev/mtd0ro`) | 16 MB | do this first; it is read-only and trivial |
| Full eMMC (`/dev/mmcblk0`) | 7.6 GB | **do this before any write** |
| `/mnt/persistent` (p3) | 32 MB | small, cheap |
| p2 recovery partition | 1 GB | the thing you are protecting |

Back up the kernel region specifically before touching it:

```sh
dd if=/dev/mmcblk0 bs=512 count=14000 of=/mnt/backup/kernel-slot.bin
```

Backups are **your device's own data, kept local.** They contain Control4 and
Intel proprietary code and must never be committed or redistributed — `.gitignore`
excludes them. They exist so a unit can be put back exactly as it was.

## CA-1

The recessed factory-restore button is **gpio1,15, not the main ID button at
gpio1,17.** U-Boot's `check_factoryrestore()` reads that pin in its init sequence,
*before* `bootcmd` runs, and boots the stock recovery kernel from SPI-NOR. The LED
goes yellow and the console prints `C4FR: Active`.

That recovery kernel's initramfs offers a **2-second `c4`+ENTER break-in to a root
shell**, and unlike U-Boot **it isn't password-gated**.

:::caution[A full factory restore doesn't clear a bad boot.scr]
Letting the recovery kernel run to completion (~3 min) reimages p2 and rewrites
the stock kernel and DTBs on p1, but it does **not** delete extra files. So a
`boot.scr` that hangs the box survives the restore and the loop continues.

You must catch the 2-second window, mount p1, and remove the file by hand.
:::

Full detail on the [CA-1 page](/ca1/#recovery-when-a-bad-bootscr-hangs-the-box).

## HC-800

The easiest recovery of any board here. Our install adds a third GRUB entry and
points `default` at it; `sda2` (the factory-restore system) and Control4's two
`menu.lst` entries are never written, and the original file is kept beside it as
`menu.lst.pre-openhc`. `ohc-flash uninstall` puts those exact bytes back.

**Return to stock** is Control4's own factory restore (entry 0, booting
`restore_fs` on `sda2`), run **exactly once**: a copy of entry 0 that
`savedefault`s back to the stock entry, plus `default saved`, with the `default`
file written and read back first. Never `default 0`: `restore.sh` re-images
`sda3`/`sda4` and reboots without touching `menu.lst`, so that restores forever.
`menu.lst` is backed up, read back after writing, and refused if either
`factorydefault` line is missing. The restore takes ~5 minutes and comes back as
stock Control4 on a new DHCP lease. Proven on a unit 2026-10-02.

On an openHC built with the `restore` feature, `ohc-restore` does it on the box,
from the same menu rewrite the flasher uses (`flasher/crates/core`, unit-tested):

| Trigger | How |
|---|---|
| `ohc-flash restore <host>` | runs `ohc-restore stock --no-reboot`, reads the verified result, then reboots the box; a box without the tool gets the same rewrite over SSH |
| Web UI → System → **Reset to stock** | iod's `cmd/system/restore` (`confirm`) → `ohc-restore stock` |
| Hold the **ID button** 10 s | `S96ohc-restore-button` → `ohc-restore watch-button` |
| Hold the **ID button** at power-on | Control4's GRUB factory-default button, unchanged |

The ID button is an input device here (`gpio-keys-polled` owns the line and
reports `KEY_F5`), so the watcher reads the key state rather than the GPIO. Same
fail-safe as the EA's: it arms only after reading the button released, so one
stuck or held through boot never starts a restore; the red Wi-Fi LED blinks while
it counts. Configured in `board.env` (`OHC_RESTORE_BUTTON_INPUT`, `_KEY`, `_HOLD`,
`_LED`). `ohc-restore status` prints the menu and a `state:` line (`openHC`,
`stock`, or `restore pending`).

A serial console on `ttyS0` at 115200 sees GRUB itself if that fails.

One way to actually brick this board does exist and is worth naming so it is
avoided: `/etc/init.d/flash-bios`. **The BIOS is field-flashable from Linux on
this board.** openHC doesn't touch it and has no reason to.

## IO Extender V1

Dual flash banks plus a recovery image, and every MTD tool already on the box.
openHC never touches any of it. The install writes a 32 MiB slot at `0x10000000`,
in the half of the 512 MiB NAND that stock never partitioned, and changes only
U-Boot's environment (see [Installing](/build/install/#io-extender-v1)).

The way back needs no serial console:

- **Automatic.** U-Boot counts openHC boot attempts in `ohc_try` and openHC clears
  it once its uplink is up. A panic (`panic=10`), a hang (U-Boot arms the hardware
  watchdog; openHC feeds it) or a box that never gets a network leaves the count
  climbing, and at 3 U-Boot runs the stock `oldbootcmd`. Tested on the unit with a
  kernel that panics on every boot: three attempts, then stock Control4.
- **On purpose.** `ohc-flash uninstall <host>` sets `bootcmd` back to stock.
- **Before any of this,** take a raw backup of every partition (`nanddump -n -o`,
  one file each). The NAND ECC is 1-bit, stored inverted; a backup read with the
  wrong ECC setting has holes in it.

The vendor's own A/B scheme is separate and still intact: `c4sys init` counts
stock boots in an I²C EEPROM, switches banks after too many failures, and falls
back to the recovery pair if both banks fail.

:::caution[`run tst` doesn't fall through on a silent TFTP failure]
If DHCP succeeds but TFTP doesn't answer, U-Boot loops forever — it ignores ICMP
port-unreachable. Recovery to stock needs **either** no DHCP (so `tst` aborts)
**or** a definitive TFTP *error* reply.
:::

## The clean-room boundary

Stock firmware is read to learn **interfaces**, the boot flow, a wire protocol,
GPIO names, which UART is which. Control4 and Intel code is not copied into
anything openHC ships. A netbooted kernel is an independent implementation written
from observed behaviour.

The genuinely unresolved tension is the vendor Android UI on the EA boards.
Keeping *Control4's* build is a redistribution problem even though it runs fine.
Options, cleanest first:

1. build our own AOSP container targeting the same graphics interface;
2. ship a non-Android UI on that plane;
3. treat the stock container as *the user's own existing files*, left in place on
   their unit and never redistributed by us.

Option 3 is fine for personal bring-up on your own box and is what the milestones
assume. **It isn't distributable.** Decide before publishing images.

The licence position on the kernel side is better than expected, every graphics
kernel module on the EA boards declares Dual BSD/GPL or Dual MIT/GPL, so the
kernel modules are redistributable. The proprietary surface shrinks to the
PowerVR userspace driver and the Android container itself. See
[the GPL drop](/shared/gpl-source/) for what Control4 does and doesn't
give us rights to.
