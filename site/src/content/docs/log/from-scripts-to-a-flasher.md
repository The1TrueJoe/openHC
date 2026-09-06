---
title: From scripts to a flasher
topic: Tooling
summary: Turning a drawer of one-off Python into something that identifies a board, refuses to guess, and installs the right image without a serial cable.
description: How the install tooling was consolidated, and the design rules it enforces.
sidebar:
  order: 10
  label: From scripts to a flasher
---

By the time four boards booted, the tooling was a drawer of single-purpose
scripts. A BOOTP responder, a TFTP server, a serial console, a shell driver, an
NFS server, an IO prober, a takeover installer. Each written to answer one
question and kept because it still worked.

That's fine for the person who wrote them and useless to anyone else. Every script
had its own idea of how to name a board, its own copy of the addressing table, and
its own assumptions about what it was pointed at.

## The rule that shaped the rewrite

Never guess which board you're talking to.

That isn't fussiness. The install paths are mutually destructive. The EA family
wants a bzImage wrapped in an Intel container written to a raw eMMC offset, the
CA-1 wants a zImage and a DTB copied onto a FAT partition, the HC-800 wants two
files on an ext3 partition and a text edit. Writing an EA image to a CA-1 doesn't
fail cleanly.

So board identity is a first-class type, and it has to work from every side of a
takeover: a unit running stock Control4 (identified from `/proc/c4board`), a unit
already running openHC (from its own `board.env`), or a unit sitting at the
bootloader shell with no operating system at all, where the only evidence is the
banner's `Type N, Rev M`.

The table of known boards carries the type and revision pairs measured against
real units. Where a variant has never been read off hardware, its ids are
explicitly absent, and detection reports an honest ambiguity instead of guessing.
An ambiguity costs a question. A wrong guess flashes the wrong image at someone's
board.

## Methods as data

The other structural decision was to separate *which* method applies from *how*
the method runs. The choice logic is pure data and testable with no hardware
attached; execution lives elsewhere because it needs I/O.

Three methods, and the summary each presents to a user:

| Method | What it does | Needs |
|---|---|---|
| **network** | over SSH into a running system: write the eMMC container and, on a secure-boot part, the MFH autoscript | nothing but an IP |
| **uboot** | copy `zImage` + DTB + `boot.scr` onto the CA-1's vfat partition | nothing but an IP |
| **serial** | drive the CEFDK shell over a console | a USB-serial adapter and the ID button |

The serial method is the fallback, not the default, and that inversion is the
whole point of the last year of work. It exists for when there's nothing running
to SSH into.

When a method doesn't apply, the API returns the reason rather than a boolean, so
a UI can explain why an option is greyed out instead of silently omitting it.

## What made a no-serial install possible

Two findings, from two different parts of the work, combine into it.

From the [EA3 takeover](/log/the-fuse-at-the-wrong-address/): the CEFDK
autoscript runs before the verifying boot path, so a stored script boots an
unsigned kernel at every power-on with no button and no host.

From the recon: the vendor's stock image has root SSH, a writable rootfs, and
access to whatever the bootloader reads next.

So the install doesn't need to interrupt the boot at all. It writes what the
bootloader will find next time, from inside the running vendor system, over the
network. The two-stage flow, write kernel and initramfs, reboot into RAM, then
write the rootfs to p1 and reboot into that — means the destructive step happens
while running from RAM, not from the filesystem being replaced.

## What being a real program bought

Consolidating into one Rust workspace with a GUI, a CLI over the same engine, and
CI producing binaries for macOS, Windows and Linux is mostly ordinary software
work. Three things came out of it that matter to the research.

**One board table.** The addressing, the type/revision pairs, the fuse state and
the peripheral flags live in one place. Previously the MAC a BOOTP responder would
answer for lived in one script and the eMMC offsets lived in another, and they
disagreed after a board got added.

**A dry run.** `plan <board>` prints what installing would do without doing it.
Given that several of these operations are only reversible via a button on the
device, being able to read the plan first isn't a luxury.

**A self-test that needs no hardware.** Most of the logic, which method suits
which identity, whether a release bundle contains the files a board needs, whether
a container header is well-formed, is decidable offline. Testing it offline means
the hardware sessions get spent on things that genuinely require hardware, which
on this project is a scarce resource. Several findings in this log needed someone
in the room to listen for a relay click or look at an LED.

## What's still manual

The ID button. Nothing in software can press it, so the tool prints a reminder and
waits, with a generous timeout, because the alternative is a tool that gives up
while you're walking to the other room.

And the first boot on any board that's never booted. Every one of the four proven
boards produced at least one failure that only appears on silicon: a cache that
needed flushing, an interrupt domain that was never created, a file operation flag
that only matters on `open()`, a pin muxed to a peripheral that isn't on the
board. A first boot gets a serial console attached, every time.
