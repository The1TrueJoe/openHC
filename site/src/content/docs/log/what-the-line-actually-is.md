---
title: What the line actually is
topic: Recon
summary: Six boxes that look like a product family. Underneath the matching cases are four unrelated computers, and working that out was the first real result.
description: Establishing what the Control4 controller line actually is, board by board.
sidebar:
  order: 1
  label: What the line actually is
---

This started with one box and one goal: run my own IO firmware on a Control4 IO
Extender and get off Control4's cloud. It grew to eight board profiles because
the first assumption was wrong.

It was a reasonable assumption. Control4 sells a controller *line* — EA-1, EA-3,
EA-5, HC-250, HC-800, CA-1, in matching cases, running matching-looking
firmware, with the same root password on every unit. (`t0talc0ntr0l4!`, and no,
that isn't the hard part.) It looks like one platform in several sizes.

## It's four platforms

Reading the units settled it, and the differences aren't incremental:

- The **EA-1 and EA-3** are Intel Atom **CE5310** set-top-box SoCs booting Intel's
  **CEFDK** out of SPI-NOR.
- The **HC-800** isn't embedded at all. It's a small x86 PC. Lite-On motherboard,
  AMI BIOS with real DMI, Atom D525, SATA SSD, **GRUB 0.97**.
- The **CA-1** is a Freescale **i.MX6 SoloLite** running stock **U-Boot**.
- The **IO Extender V1** is a TI DaVinci **DM355**, ARMv5, running a 2.6.28 kernel
  from 2009.

Four architectures, four bootloaders, nothing shared but a userspace stack and a
naming convention. Control4 names board revisions after Lego sets — `ninjago` for
the EAs, `garmadon` for the EA5, `emmet` for the CA-1, `hammer` for the IO
Extender. Charming, and also the fastest way to tell which platform a file
belongs to.

## And two of them are the same computer

That's the more useful half of the result. The EA-1 and EA-3 look like different
products and aren't. Same CE5310, same ~1.5 GB, same eMMC layout, same CEFDK
build, byte-identical IO-MCU firmware, same codename. The kernel version string
matches exactly: `3.12.74 #8-140-ninjago.1`.

The entire difference is which peripherals are populated, plus one fuse.

That mattered right away, because it decides how the repo is shaped. EA3 support
is a **board profile**, not a port. Each EA variant lists what it has in a small
`ohc.features` file and shared feature sets supply the config, so adding a
variant is a short list of real differences instead of a copied tree that drifts
out of sync six months later.

## The board straps say so out loud

The EA family exports seven GPIO lines as `board_id0..6`, and they're not an
opaque board number. They split cleanly into the two values the kernel already
publishes:

```
board_id0..3  ->  revision   1,0,0,1 = 9   ( = /proc/c4board/revision )
board_id4..6  ->  type       0,1,0   = 2   ( = /proc/c4board/type     )
```

So type 1 is an EA1 and type 2 is an EA3, with the low nibble carrying the PCB
revision. You can pick a board profile off those pins without parsing anything.

The HC-800 uses the same three strap lines for something else entirely. They read
`100b` = 4, and that's the **board revision**, with `type` sitting at 0. The CA-1
is also type 0 and identifies itself by name. So the strap convention is an
EA-family thing, not a line-wide one, which is the sort of detail that produces a
confidently wrong auto-detect if you assume otherwise.

## What the shared root password actually buys

Root over SSH on a stock unit is easy. It's worth being precise about what that
gets you, because it's less than it sounds.

It gets you **recon**. `/proc/config.gz` on the boards that carry it, the vendor's
init scripts, which process holds which file descriptor, the GPIO alias names,
the firmware manifests, and the ability to read a fuse register through
`/dev/mem`. Nearly every fact on this site was first seen that way.

It does not get you control of the boot chain, which is the actual objective. The
kernel on an EA board isn't a file on the rootfs at all. It lives in a raw eMMC
region managed by the bootloader's own header table, so replacing it isn't an
edit, it's a flash write. And the bootloader is in a different chip.

That last point is the safety net this whole project leans on. On the EA boards
CEFDK lives in SPI-NOR, physically separate from the eMMC, so destroying the eMMC
doesn't brick the unit. The bootloader survives to reflash it, netboot a kernel,
or take a new image over YMODEM.

Knowing that before writing anything is what made it reasonable to keep going.

## Getting the sources first

Control4 publishes GPL source, and the old scheme is still live on S3:

```
http://update.control4.com/open_src/<VERSION>-res/src/<name>+patches.tar.gz
```

Directory listing is off, but a per-version XML index lists every file with its
size and MD5. The DM355 unit runs OS 2.9.1 and that directory 403s, but
`2.9.0.525559-res` and `2.10.0.540110-res` both return 200 and carry identical
DM355 sources.

Each archive is a pristine upstream tarball plus a `patches/` directory of
Control4's changes, which is a better disclosure than most. It included the DM355
board file and the custom IO drivers as patches. And, as
[the DM355 port](/log/resurrecting-a-dead-soc/) gets into, left out a few
driver bodies it shouldn't have.

The newer drops are at <https://open-source.control4.com/>, unauthenticated.
`OS-3.3.1.zip` is 2.59 GB and is the last one covering the EA-1; OS-4.x moved to
i.MX and dropped both tarballs. What's usable in it's
[its own page](/shared/gpl-source/). Short version: the kernel side is
redistributable and doesn't compile as shipped, the bootloader is proprietary on
every single file, and graphics aren't in there at all.

With that downloaded, the question became the one everything else depended on.
Can you get a prompt out of the bootloader, or is this a sealed box that only
ever runs what Control4 signed?
