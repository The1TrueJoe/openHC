---
title: The page that wrapped on itself
topic: Flash
summary: Making the EA boot flash writable was supposed to be the easy part. Erase worked, reads worked, and the first real write corrupted the one structure that bricks past the recovery button — because spi-nor mis-read the chip's page size and every program folded in half.
description: Bringing up a writable /dev/mtd0 on the EA boot SPI-NOR, the SFDP page-size misread that corrupted the MFH, and the kexec that made it recoverable.
sidebar:
  order: 12
  label: The page that wrapped on itself
---

An in-place install and a software return-to-stock on the EA both need the same
thing: the boot SPI-NOR has to be writable from Linux. The stock image sees it
read-only as `nmyx25`; openHC needed a real `/dev/mtd0`.

The chip hangs off the CE5300's **dedicated** boot-flash controller — PCI
`8086:08a0`, a separate block from the pxa2xx SSP the BCM switch rides — so it
needed its own driver. `spi-ea-ce5xx` is a mainline-API port of Intel's
out-of-tree `ce5xx_spi_flash.c`, rewritten against `spi-mem` so the in-tree
`spi-nor` driver detects an S25FL127S by JEDEC id and drives it. It came up:
`/proc/mtd` showed a 16 MB `mtd0`, reads matched the known 16 MB backup
byte-for-byte, and an erase test cleared a 64 KB block to all `0xff`. Reads good,
erase good. The write looked like a formality.

## The one structure you must not corrupt

The first real write was the install itself: append a single `script` item to the
Master Flash Header, the item-table at `0x80000` that CEFDK reads to find what to
boot. It is SHA-256 protected, and a bad write to it bricks **past** the recovery
button — the native button doesn't rewrite it, so recovery would mean clipping an
external programmer onto the flash.

It corrupted. The read-back after the write didn't match what was written. Not
garbage everywhere — the block was half right.

## Erase works, read works, program is wrong

The pattern was specific. The first 256 bytes of each page were wrong and the
second 256 bytes were the data that should have been in the first. Every `0x100`
boundary, the halves had traded places, and the second half had overwritten the
first. A clean `-0x100` fold.

The driver wasn't issuing those offsets. `spi-nor` was. It mis-read the chip's
SFDP parameter table and concluded the part took **512-byte** page programs with
**4-byte-address** opcodes. The real S25FL127S on this board programs **256 bytes**
a page. So `spi-nor` handed down a single 512-byte `PAGE_PROGRAM_4B`, the chip
wrapped it at its own 256-byte page boundary, and the back half of the buffer
landed on top of the front half. The program *completed* — no error, WIP cleared
— it just wrote the wrong thing. Erase and read had no page granularity to get
wrong, which is exactly why they'd looked fine.

The fix lives in the controller's `exec_op`, where the opcodes actually go out:

- **fold the 4-byte opcodes back to 3-byte** — `0x12`→`0x02` (page program),
  `0xdc`→`0xd8` (64 KB erase), `0x13`→`0x03` and `0x0c`→`0x0b` (read, fast read)
  — and drop to 3 address bytes, which is all a 16 MB part needs;
- **split every program on the chip's 256-byte page boundary**, re-issuing `WREN`
  and waiting for `WIP=0` between chunks, so a program that `spi-nor` thinks is one
  operation becomes however many 256-byte-aligned writes the chip truly wants.

With that, the read-back matched.

## Why a corrupt MFH was still recoverable

It shouldn't have been. A bad MFH is the brick-past-the-button case. It was
recoverable because of one property of how openHC boots a kernel at all:
**`kexec` is a soft load. It never reads the MFH.** CEFDK consults the MFH on a
cold boot; `kexec` jumps straight into a kernel already in RAM.

So the corrupt box still had a running kernel, still had the writable `mtd0`, and
could `kexec` into a *fixed* kernel without CEFDK ever looking at the broken
header. From there, `ohc-restore restore-mfh` rewrote the MFH block from the 16 MB
backup — erase, write through the now-correct program path, read-back-verified —
and only then a full cold reboot, which found a valid header again.

## The lesson, stated plainly

**Erase working does not mean program works.** They are different command paths on
the chip and different code paths in `spi-nor`, and the one that's hardest to get
right is the one that does the damage. The order that follows from that is now a
rule: never run a live `mtd` **write** against a structure you can't afford to lose
until the program path has been proven on bytes you can. Every write `ohc-restore`
does is erase-then-write-then-**read-back-verify** for the same reason — the write
returning success proved nothing here.
