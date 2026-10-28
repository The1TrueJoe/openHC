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
  + board/ea/common/common_defconfig   EA family only
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

## Which Buildroot

openHC builds against **Buildroot master, pinned to a commit** (`04b8e19`, from
2026-10-26), not a release. The pin is `BUILDROOT_REF` in `ci.yml`,
`sgx-module.yml`, `sgx-wpe.yml` and `build/Dockerfile`. The fetch takes a SHA or
a tag, so moving to the 2026.11 tag when it ships is a one-line change in each.
This moved from 2026.02.3 in openHC #82.

Whatever upstream has fixed, openHC stops carrying. The move to master dropped:

- **The shairport-sync and plistutil forks** (`packages/ohc-shairport-sync`,
  `packages/ohc-plistutil`). Buildroot ships shairport-sync 5.6, which has
  every commit the fork pinned, plus `host-libplist`.
- **The nqptp fork** (`packages/ohc-nqptp`). AirPlay 2 on shairport-sync 5.6
  needs nqptp 1.2.9, the release that serves several receivers on one host.
  Buildroot had 1.2.4, so openHC sent the 1.2.9 bump upstream. It was applied
  to master as `04b8e19`, and that commit is the pin, so no local nqptp patch
  is left.
- **glibc BZ 33367** (`board/ea/common/patches/glibc`). Fixed in glibc 2.43,
  and master ships 2.44.
- **`buildroot-patches/0002`** (gcc libsanitizer). gcc 14.4 and 15.3 no longer
  include `linux/scc.h`.
- **`buildroot-patches/0003`** (harfbuzz without cairo). It was only needed
  until the next from-scratch build.

What is still carried:

- **`build/buildroot-patches/0001` and `0004`, both wpewebkit.**
  - `0001` swaps the wpewebkit dependency from `wpebackend-fdo` to `libwpe`. A
    test build on 2026-10-27 showed it is no longer needed: the EA's i386 tree
    builds with `wpebackend-fdo` present, against the SGX EGL, and still
    selects the PowerVR backend. That costs about 264 KB of unused libraries.
    It is to be dropped in a follow-up, which boots on an EA and the HC-800
    first.
  - `0004` adds the WPE Platform API and its DRM platform as Buildroot options.
    The HC-800's display needs it, Buildroot has no equivalent, and it is the
    one candidate to send upstream.
- **The two shairport-sync D-Bus patches** (`board/common/patches/shairport-sync`,
  on `BR2_GLOBAL_PATCH_DIR` for the EA family and the HC-800). They give a
  second receiver its own bus name and make a D-Bus volume set the next
  session's starting volume. Upstream 5.6.2 does neither yet.
- **An `S90nqptp` overlay stub**, which replaces the init script Buildroot's
  nqptp installs. audiod and `S99shairport-sync` start the one shared nqptp
  themselves, the same way `S95mpd` is handled.

The move also changed configuration:

- Kernel headers went to `CUSTOM_7_1`, matching the 7.1.8 kernel.
- The EA external toolchain went to GCC 15 with 7.1 headers. GCC 15 defaults
  userspace to C23.
- `gpu-wpe-i386` uses the bundled 6.18 headers, because 6.19 is no longer
  bundled.
- The EA family now gets the shared AirPlay 2 build like the HC-800, instead of
  classic AirPlay 4.3.7.

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
