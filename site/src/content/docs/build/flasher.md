---
title: The flasher
description: The tool that identifies a board, refuses to guess, and installs the right image.
sidebar:
  order: 3
---

`ohc-flasher` is a desktop application with a CLI over the same engine. It exists
because the install paths for these boards are **mutually destructive**, writing
an EA image to a CA-1 does not fail cleanly, and because a drawer of one-off
scripts is fine for the person who wrote them and useless to anyone else.

## The rule it enforces

**Never guess which board you are talking to.**

Board identity is a first-class type, and it has to work from every side of a
takeover:

| Unit is running | Identified from |
|---|---|
| stock Control4 | `/proc/c4board` |
| openHC | `/opt/ohc/board.env` |
| nothing — sitting at the CEFDK shell | the banner's `Type N, Rev M` |

The known-board table carries type and revision pairs **measured against real
units**. Where a variant has never been read off hardware, its ids are explicitly
absent, and detection then reports an **honest ambiguity rather than guessing**.

An ambiguity costs a question. A wrong guess flashes the wrong image at someone's
board.

## Methods as data

*Which* method applies is separated from *how* it runs, so the choice logic is
pure data and testable with no hardware attached.

| Method | Summary | Families |
|---|---|---|
| `network` | over SSH — no serial, no button (default) | EA |
| `uboot` | copy files to the vfat partition — no serial | CA |
| `serial` | via the CEFDK shell — needs a console and the ID button | EA |

When a method does not apply, the API returns **the reason** rather than a
boolean, so the interface can explain why an option is greyed out instead of
silently omitting it.

The single fact the installer branches on is whether a board's secure-boot fuse is
blown, because a secure-boot part can't use the normal verifying boot path and
needs the [autoscript](/ea/secure-boot/) instead.

## CLI

```
ohc-flash <command>

  discover                 find Control4 units on the network
  identify [HOST]          say what a unit is (auto-discovers if omitted)
  boards                   list known boards
  plan <board>             show what installing on <board> would do
  validate <dir|zip>       check a release's images
  install [HOST] --images <dir|zip> [--dry-run] [--yes]
  rootfs  [HOST] --images <dir|zip> [--yes]
  wrap <bzImage> <out> [--header FILE]
```

Any command taking `[HOST]` also accepts `--password` for a controller whose root
password isn't the factory default.

`install` is the two-stage flow: it writes the kernel and initramfs, reboots into
RAM, then writes p1 and reboots into it. `--no-wait` stops after stage one;
`rootfs` is stage two on its own, for a box already RAM-booted.

Three commands are worth knowing about specifically:

**`plan`** prints what an install would do without doing it. Several of these
operations are only reversible via a button on the device, so reading the plan
first isn't a luxury.

**`validate`** checks a release bundle offline — whether it contains the files a
given board needs, and whether a container header is well-formed. No hardware
required.

**`wrap`** does the CEFDK container step on its own, for anyone building a kernel
outside the normal flow.

There is also a self-check that needs no hardware at all. Most of the logic is
decidable offline, and testing it offline means hardware sessions get spent on the
things that genuinely need hardware, which on this project means someone in the
room listening for a relay click.

## Releases

CI builds the flasher for macOS (a universal binary in a `.app` bundle), Windows
and Linux on every push, and attaches them to GitHub releases. Image bundles are
built per board and attached to the same releases, so the flasher can pull the
latest image for a board directly.

The binaries are **unsigned**, there's no Apple or Windows signing certificate —
so first launch needs a right-click → Open on macOS and "More info → Run anyway"
on Windows.

Installed units carry their version in `/etc/openhc-release`, stamped at build
time, so the flasher can tell whether a box is up to date against the latest
release.

## What it still cannot do

**Press the ID button.** Nothing in software can, so the tool prints a reminder
and waits with a generous timeout — the alternative is a tool that gives up while
someone is walking to the other room.
