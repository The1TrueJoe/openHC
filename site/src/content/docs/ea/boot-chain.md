---
title: Boot chain
description: How an EA board boots, from SPI-NOR through CEFDK to a kernel in a raw eMMC container.
sidebar:
  order: 4
---

All of this is read off live units over the serial console; none of it's
inference from documentation.

```
SPI-NOR ──► Intel CEFDK ──► kernel (raw eMMC) ──► initramfs /init ──► one of four boot modes
```

CEFDK announces itself on `ttyS0` at 115200 and reports the board before doing
anything else:

```
CEFDK Version : CE5300 (SMP enabled)     Boot Mode : SPI-NOR (STRAPS)
Board         : Type 1, Rev 5            MAC       : 00:0f:ff:1a:fc:a9
8051 Firmware : C0-1.0.53                Silicon   : D0 (PCI), SKU 0x08F
```

The single most important structural fact: **CEFDK lives in SPI-NOR, physically
separate from the eMMC.** Destroying the eMMC doesn't brick the unit, the
bootloader survives to reflash it, netboot a kernel, or take a new image over
YMODEM. See [recovery](/shared/recovery/).

## CEFDK cannot be interrupted on a normal boot

Tested directly: Ctrl-C, ESC, space, CR, `x` and `c4` sent continuously at
115200 through the entire CEFDK window across a reboot. No prompt, no pause.
CEFDK goes straight from its banner to `Executing Control4 Normal Boot Mode`.

The reason is narrower than "no shell". There *is* a break-in prompt; it's
gated twice:

- `userInit()` only calls it after `c4_id_button_is_pressed()` reports
  manufacturing mode. **No button, no prompt.**
- The build on these units uses `shellOnPassword` (Control4 patch
  `0035-use-password-to-enter-shell`), which reads a line, SHA-256's it and
  compares against a 32-byte digest baked into the image, beginning
  `ec 89 70 13 ad f9 …`. We do not have the preimage.

The CA-1's U-Boot uses **the identical digest**.

The way to an unlocked shell doesn't involve the password at all — see
[bootloader access](/ea/bootloader-access/).

## Boot mode selection

`/init` picks one of four paths, and every decision reads `/proc/cmdline`:

| Condition | Mode |
|---|---|
| `mfgtest` in cmdline | `/mfg_prog` |
| `recovery` in cmdline | `/recovery_boot` |
| no `/dev/mmcblk0p1` **and** no `nfsroot` in cmdline | `/dev_prog_boot` |
| otherwise | `/normal_boot` |

The stock cmdline, set by CEFDK:

```
console=ttyS0,115200 rw root=/dev/mmcblk0p1 rootwait ip=none memmap=exactmap
memmap=128K@128K memmap=1585M@1M vmalloc=586M androidboot.hardware=intelce
```

:::danger[`/dev_prog_boot` is a factory programmer, don't run it]
It mounts NFS, which is the tempting part. Its main body, read off the device:

```
format_emmc_8g            <-- wipes the entire 8 GB eMMC
create_filesystems
mount_nfs
update_factory_kernel
extract_rootfs
install_boot_kernel
install_spiflash_kernel   <-- reflashes the SPI-NOR bootloader
reboot -f
```

Running it to "try netboot" formats the eMMC and reflashes both the kernel and
the SPI-NOR from whatever the NFS server happened to be serving. The name is the
warning: *development **programmer** boot*.
:::

## The initramfs escape hatch

```
Type 'c4' followed by [ENTER] within the next 2 seconds to stop boot and
break into initramfs.
```

Typing `c4` drops to a BusyBox root shell **before** the rootfs is mounted, with
the eMMC reachable as `/dev/mmcblk0` and `dd`, `mount`, `tar` and `vi`
available. Leaving the shell continues the normal boot, so it is
non-destructive.

`exit` **panics the kernel**, it kills PID 1. Run `/normal_boot` to continue
instead.

This produces an asymmetry that governs the order of all work on these boards:

- a broken **rootfs** is recoverable over serial — the initramfs still runs;
- a broken **kernel** is **not**, because the initramfs providing the escape is
  part of the kernel image that failed to load.

Recovering from a bad kernel needs external hardware. Which is why netboot —
which touches no flash at all, is the right way to prove custom code before
anything is written.

## Where the kernel actually lives

CEFDK does **not** read the kernel from a filesystem. It reads it from a raw
offset near the start of the eMMC, and the boot log states it outright:

```
Read Kernel Size Successfully from emmc address(0x00000200)! Kernel is (7009216)(0x006af3c0) bytes
Successfully read (7009216) bytes of kernel into memory at (0x00f00000) from emmc address (0x00000400).
VERIFY_S3(kernel bzImage): PASS
```

Verified byte-for-byte on the device:

```
eMMC 0x200  u32 BE   container size          -> 00 6a f3 c0  = 7,009,216
eMMC 0x400  CEFDK container header, 0x580 bytes
              +0x10  0x8086                  Intel vendor id
              +0x14  25 05 22 20             build date, matches CEFDK's 05/25/22
              +0x28  0x00000580              offset from container start to the bzImage
eMMC 0x980  bzImage                          "HdrS" lands at 0x980+0x202 = 0xb82
```

So the recipe for a custom kernel is: build a bzImage, prepend the 0x580-byte
container header, write it at raw offset `0x400`, and write the new total size
as a **big-endian u32** at `0x200`.

`board/ea-common/post-image.sh` does the wrapping. Note that
[on a secure-boot part this path is rejected](/ea/secure-boot/) and the
kernel goes in a different place entirely.

The stock kernel isn't a file on the rootfs, searching for `bzImage` or
`vmlinuz` comes up empty. It lives in a bootloader-managed raw region, which is
another reason netboot is the sane iteration path.

## The copy window

`bootlinux` copies the protected-mode kernel to `0x100000`. Its own loader code
lives at about `0x812f07`, with the final jump near `0x8130be`. **If the copy
reaches that, it overwrites the loader mid-copy and crashes on return**, with no
diagnostic, because the code that would print one has just been replaced.

So the protected-mode kernel must stay under about **7.0 MB**
(`0x813000 − 0x100000`). Precisely: CEFDK's `bootlinux` copy window measures
**7,417,856 bytes**.

This governs every configuration decision on the EA family. Measured budget on
ea3-v2, x86_64:

| Build | bzImage | Headroom |
|---|---|---|
| features = `emmc` | 7,169,024 | 248,832 (3.4 %) |
| + unused NIC and ISO9660 trims | 7,037,952 | 379,904 (5.1 %) |
| with the `sgx` feature | ~6.47 MiB on i686 | ~1.61 MiB |

`.text` is 9.6 MB and `.rodata` 3.4 MB. The configuration is already lean, no
netfilter, IPv6, Bluetooth, SCSI/ATA/MD, DRM, sound, media, ftrace or debug info.
**A 64-bit kernel on this board simply costs about 7 MB.**

### What that forces

Features written as `=y` land in the image whether the board uses them or not.
`CONFIG_MODULES` is on and exactly one symbol in the whole configuration is `=m`,
against 1370 that are `=y`.

So features that aren't needed to *reach the rootfs* should come back as
**modules**, not built in. Only e1000, sdhci/mmc, ext4, serial, PCI and ACPI must
be built in. If a genuinely large kernel is ever unavoidable, the escape hatch is
a small bootstrap kernel that `kexec`s the real one from p1, kexec has no such
limit.

## Two board quirks you will hit

**The UART is at 921600, not 115200.** The CE5310's legacy `0x3f8` UART clock is
**8× standard** (14.7456 MHz), so a mainline kernel asked for 115200 drives the
wire at `8 × 115200 = 921600`. CEFDK knows the real clock and prints fine at
115200; the kernel does not. Read the kernel console at 921600 — output is clean,
typing back is overrun-prone. Proper fix: set the legacy `uartclk`, or use
`console=ttyS0,14400` (divisor 8 → true 115200 on the wire).

**PCI BARs come up unassigned.** CEFDK only programs the BARs for what it uses,
so the kernel finds `ath9k` and `e1000` with BAR 0 = `0x00000000` and cannot
`ioremap` them. Boot with **`pci=realloc,nocrs`**, baked into `CONFIG_CMDLINE`
with `CMDLINE_EXTEND`.

There's a related trap on the 8250 driver. The four legacy ISA declarations
consume every 8250 slot, so the **real PCI UARTs** can't register at all:

```
Couldn't register serial port 0, irq 25 ... -28    (ENOSPC)
```

This is why every hunt for the Zigbee radio on `ttyS1..3` came back empty —
those are the legacy ports, they enumerate with `irq = 0`, and a port with no
IRQ transmits fine but never receives. `SERIAL_8250_NR_UARTS` is raised from 4
to 8.

## Recovery nets, all proven on hardware

| Net | Trigger | Result |
|---|---|---|
| Recovery kernel | recovery button at power-on | factory kernel from eMMC + p2, full reimage |
| initramfs shell | type `c4` in the 2 s window | BusyBox before rootfs mounts |
| Watchdog | any hang or panic | CEFDK's 300 s watchdog reboots the box |

A confused CEFDK state after `bootkernel -b` experiments is cleared by a plain
power-cycle with no button held.
