---
title: Recovery
description: Read this before writing anything to a device — what each button does, and the one region on each board that must never be touched.
sidebar:
  order: 2
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
| **IOX v1** | **dual flash banks plus a recovery image**, and bring-up never writes flash at all | the recovery bank |

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

The easiest recovery of any board here: **change one digit.** Our install adds a
third GRUB entry and moves `default` from `1` to `2`. Setting it back to `1` boots
the vendor image again, and the vendor root on `sda4` and the factory-restore
image on `sda2` are never written.

Keep a copy of the original file, the install saves `menu.lst.stock` alongside it.

A serial console on `ttyS0` at 115200 sees GRUB itself if that fails.

One way to actually brick this board does exist and is worth naming so it is
avoided: `/etc/init.d/flash-bios`. **The BIOS is field-flashable from Linux on
this board.** openHC doesn't touch it and has no reason to.

## IO Extender V1

Dual flash banks plus a recovery image, and every MTD tool already on the box.
Bring-up writes no flash at all — `run tst` RAM-netboots a kernel over TFTP.

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
