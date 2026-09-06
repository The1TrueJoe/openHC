---
title: The fuse at the wrong address
topic: Secure boot
summary: I measured secure boot, decided it was off, and was right about one board and wrong about the line. Then took the EA3 over anyway, without defeating the check.
description: The secure-boot measurement that was correct and useless, and the CEFDK autoscript takeover it led to.
sidebar:
  order: 5
  label: The fuse at the wrong address
---

This is the one about being wrong in the most expensive way available:
confidently, with evidence, and for weeks.

## The measurement

CEFDK contains a real signature chain. `VERIFY_S2`,
`VERIFY_S3(kernel bzImage)`, `VERIFY_S3(initrd)`, each with `PASS` and `FAIL`
variants, an RSA public modulus for stage1→stage2, and a Control4 patch that sets
`CFG_KERNEL_AUTHENTICATION=1` and swaps in Control4's own key for stage2→kernel.

The machinery is there. The question was whether it's enforced, and enforcement is
hardware-gated. From `brd_gen5/sec_boot_linux.c`:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x14) & (2<<21)) ||  // SEC_BOOT_FUSE
    cp_strap_sts_0().strap.sec_boot)                        // SEC_BOOT_STRAP
```

That register is readable from a running Linux, since the stock kernel has
`CONFIG_STRICT_DEVMEM` unset. So I reproduced CEFDK's own check against
`/dev/mem`:

```
DFX device      : 0000:01:0b.7      (AV_BUS:0b.7, per CEFDK's pciR32)
BAR0            : 0xdf8f0000
reg[BAR0+0x14]  : 0x00000000
SEC_BOOT_FUSE   : 000 (bits 23:21)
secure-boot fuse: CLEAR
```

Clean, reproducible, sourced from the vendor's own code. Signature enforcement is
off; a self-built unsigned kernel should boot. And on the EA1 it did. An unsigned
7.1.8 booted through `bootlinux` and came up on Wi-Fi, exactly as predicted.

I kept two caveats honest at the time: the register reads entirely zero, and the
`SEC_BOOT_STRAP` half of the condition lives in a strap-status register I hadn't
located from Linux. Both were the right things to flag. Neither was the actual
problem.

## The EA3 refuses

Write an openHC kernel into the eMMC container, power on an EA3 normally, and:

```
VERIFY_S3(kernel bzImage): FAIL
```

then a `SOFT_HANG` and a watchdog boot loop. The factory-restore button recovers
it fully, which is the only reason this was a bad afternoon and not a dead board.

## Why the measurement was useless

I read `dfx_mbar + 0x14`. That's the pre-patch Intel location.

Control4's CEFDK patch `0022-s3-boot-verify-fix` moved the check to
`dfx_mbar + 0x60` bit 0, and dropped the `rev_id == 4` escape hatch the original
had:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x60) & BIT(0)) ||  /* SEC_BOOT_FUSE=1 */
    cp_strap_sts_0().strap.sec_boot)
    return true;                                            /* enforced */
```

So `+0x14 == 0` says nothing whatsoever about the shipping check. I'd read the
source, found the register, reproduced the logic, and read the version of the
logic this firmware doesn't use.

The lesson isn't "check your offsets". It's that I validated against upstream
source when the shipping binary was patched, and the patch series was sitting in
the same archive I'd already downloaded. The correction stays in full on the
[GPL source page](/shared/gpl-source/) rather than getting quietly edited
away, because a wrong conclusion that survived weeks of work is worth more as a
warning than a clean page is as a reference.

There's a secondary lesson too. The EA1 result was *not* wrong. An EA1 (board v1)
does boot unsigned kernels on the normal path. The error was generalising one
board's measurement to a family. The fuse was probably blown starting at some
board revision, which also means an EA3 v1 may behave like the EA1, and nobody
has checked.

Signing our way out isn't available either. Control4 replaced Intel's RSA key with
their own, and no private key exists outside the vendor.

## The door that was open the whole time

Here's what redeems the afternoon. The fuse blocks exactly one thing: CEFDK's
normal boot path, `hndBootKernel`, which is `bootkernel`.

`bootlinux` still doesn't verify. It never did. It checks `0xAA55` and `HdrS` and
jumps.

And `userInit()` runs `runAutoScript()` *before* the verifying normal-boot path.
CEFDK has a stored-script facility, as an MFH slot type in SPI-NOR, and it
executes ahead of the thing that rejects us.

So the takeover is CEFDK's own autorun feature, which is the exact analogue of the
CA-1's `boot.scr` trick done CEFDK's way:

```
emmc rd <gap> <ram> <size>          ; raw-read our kernel from eMMC into RAM
cache flush
ord4 <linuxKernelBase> = <ram>
bootlinux "root=/dev/mmcblk0p1 …"   ; unverified boot of an unsigned kernel
```

Stored once from the shell with `script on`, undone with `script off`. An EA3 now
self-boots our unsigned 7.1.8 at every power-on. No button, no host, no network,
rooted on p1 as a real read-write ext4 filesystem.

## Two things that had to be learned on hardware

The autoscript didn't work first time, and both failures were silent.

**The cache.** `emmc rd` DMAs the kernel into RAM but doesn't invalidate the CPU
cache, so `bootLinux` read stale bytes and died with `Invalid or missing kernel`.
The shipping shell has a `cache` command, and `cache flush` reconciles it. It's a
reduced command set, no `msr`, no `mmc`, but `cache`, `emmc`, `tftp`, `ip`,
`ord[2|4]`, `bootlinux`, `script`, `sha`, `md5`, `ymodem`, `b53` and `strap` are
all there. Run `help` rather than assuming.

**The two eMMC addressings disagree.** Linux's `/dev/mmcblk0` and CEFDK's `emmc`
command don't agree on the raw gap's byte offset. The container at `0x400` happens
to match; `0x800000` doesn't. So the kernel has to be installed with CEFDK's own
`emmc wr`, not Linux `dd`. Partition p1 is a real partition and `dd` is fine for
it; only the raw gap needs the bootloader's addressing.

## The layout this establishes

Everything is reversible with the restore button, and p2 is never written:

- the signed stock kernel stays in the container as a `script off` recovery;
- our kernel lives in the ~25 MB raw gap between the container and p1;
- our ext4 rootfs is on `mmcblk0p1`;
- the autoscript is an MFH item in SPI-NOR.

Two more results came out of that first real boot. `e1000` links, and the eMMC
needed nothing at all, mainline `sdhci-pci` binds `8086:070b` by class, so the
board that looked like it would need a vendor MMC patch enumerates
`mmcblk0 p1 p2 p3` and mounts read-write with stock drivers.

The subsystem that actually fought back wasn't the storage or the network. It was
a microcontroller that wouldn't say a word.
