---
title: Building an image
description: Two commands, in Docker, with nothing installed on the host.
sidebar:
  order: 1
  label: Building
---

Buildroot cross-compiles a musl toolchain, a modern kernel and a BusyBox rootfs.
It runs in a container so nothing lands on the host.

## Prerequisites

Docker, and Python 3 for the netboot and serial tooling. That's it for building.
Installing onto a unit needs more depending on the board, see
[installing](/build/install/).

## Two commands

```sh
make image BOARD=ea3-v2     # build the kernel + rootfs (first run is slow)
make netboot BOARD=ea3-v2   # serve it and boot it
```

`BOARD` defaults to `ea3-v2` and selects the defconfig, the kernel fragments, the
rootfs overlay, the image name, the MCU firmware profile and the netboot
addressing together.

Supported boards:

```
ea1-v1  ea1-v2  ea1-v2-poe  ea3-v1  ea3-v2  ioxv1  ca1  hc800
```

Run `make help` for the per-board notes it prints, several boards have a
different install path and the Makefile says so rather than silently doing the
wrong thing.

## How the configuration is assembled

Buildroot has no include mechanism for defconfigs, so `build/build.sh`
concatenates them and feeds the result in as `BR2_DEFCONFIG`:

```
board/common/common_defconfig          every board
  + board/ea-common/ea-common_defconfig   EA family only
  + features listed in ohc.features       wifi, emmc, switch, audio, sgx
  + board/<board>/<board>_defconfig       the board's own differences
```

**Kconfig takes the last assignment**, so a board extends or overrides shared
settings simply by coming after them, and each board file stays a short list of
genuine differences rather than a full copy that drifts.

The same layering applies to the kernel: `common.fragment`, then per-feature
fragments, then the board fragment. Ordering matters and has bitten before — the
[SGX feature](/ea/graphics/) sets `CONFIG_DRM=y` while `common.fragment`
sets `# CONFIG_DRM is not set`, and it works only because feature fragments are
appended *after* the board file.

## Parallelism and memory

`build/build.sh` caps Buildroot's parallelism by the VM's RAM at roughly
2.5 GB per job. If the toolchain still gets OOM-killed — Docker Desktop defaults
to a small VM and GCC's big translation units are memory-hungry, force it down:

```sh
make image BOARD=ea3-v2 JOBS=1
```

## What comes out

`output/images/`, and what is in it depends on the board:

| Board | Artifacts |
|---|---|
| EA family | `bzImage`, `rootfs.cpio.gz`, `rootfs.ext2`, and `openhc-<board>-kernel.img` — the bzImage wrapped in the CEFDK container |
| CA-1 | `openhc-ca1-zImage`, a DTB, a `boot.scr` |
| HC-800 | `openhc-bzImage`, `openhc-initrd.gz`, and `menu.lst.openhc` — the GRUB stanza to paste in |
| IOX v1 | a legacy `uImage` with an appended DTB |

The per-board `post-image.sh` scripts also **print the exact install steps** for
the boards whose install is manual. On the HC-800 in particular, running
`make image BOARD=hc800` is the fastest way to get the copy-paste commands.

## The dashboard

`webd` — a Rust server with an embedded React UI, is built on the host rather
than in the container, because it cross-compiles with `rust-lld` and needs no
cross-binutils:

```sh
make webd BOARD=ea3-v2
```

It stages into `board/common/rootfs-overlay/opt/ohc/bin`, so the next
`make image` bundles it. That directory is **gitignored**, so on a fresh clone it
is empty and no dashboard ships until you run this. If a browser on the box gets
`Connection refused`, check that before suspecting anything else.

Needs rustup with the board's target added, plus node and npm.

## The IO-MCU firmware

```sh
make mcu BOARD=ea3-v2
```

Builds the clean-room TM4C firmware with an `arm-none-eabi` toolchain. EA family
only — the Makefile refuses on other boards and explains why. The board profile is
compile-time, in `board_profile.h`.

## CI

Two workflows. `images.yml` builds every affected board in parallel and attaches
the bundles to releases; `flasher.yml` builds the desktop flasher for macOS,
Windows and Linux.

The images workflow runs Buildroot **directly on the runner** rather than through
the Dockerfile, deliberately: the Dockerfile keeps sources off a developer's
machine, but in CI it actively hurts, because Buildroot's `dl/` and `output/` live
in BuildKit cache mounts and cache mounts aren't exported by any GitHub cache
backend. Run it natively and `actions/cache` can hold the one thing worth holding
— the ~GB of upstream tarballs every board re-downloads, keyed so all eight matrix
jobs hit the same entry.

Toolchains are deliberately **not** shared between boards. Sharing one means an
external-toolchain tarball and a second pipeline to maintain; the repository is
public, so runner minutes are free, and parallel matrix jobs already give the
wall-clock win.

A change under `board/<board>/` builds only that board. A change to `board/common`,
`packages/`, `build/`, the flasher or the Makefile builds everything. A docs-only
change builds nothing.
