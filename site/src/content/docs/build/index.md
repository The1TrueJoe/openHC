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

## Natively on Linux

The Docker build keeps everything off the host. On a Debian or Ubuntu machine
the CI build also runs directly, which is faster and resumes cleanly:

```sh
build/deps.sh --kexec        # host packages, once (--kexec: the hc800's 32-bit kexec)
make native BOARD=hc800      # = build/image.sh hc800
```

It also needs rustup and node/npm for the daemons, about 8 GB of RAM and tens
of GB of disk. Buildroot is fetched to `~/buildroot` (`BUILDROOT_DIR`
overrides), the tree lands in `output/build/<board>/` and the flasher bundle,
`openhc-<board>-<version>.zip`, in the repo root. CI's optional extras (the EA3
DSP program, the EA on-panel browser sysroots) are left out: the image ships
without them.

## How the configuration is assembled

Buildroot has no include mechanism for defconfigs, so `build/build.sh`
concatenates them and feeds the result in as `BR2_DEFCONFIG`:

```
board/common/common_defconfig                 every board
  + board/ea/common/common_defconfig          EA family only
  + features listed in ohc.features           wifi, emmc, switch, audio, sgx
  + board/ea/<model>/defconfig                the model (EA only)
  + board/.../<board>/<board>_defconfig       the board's own differences
```

EA boards live at `board/ea/<model>/<board>/`; `build/board-layers.sh` resolves
the layers for both build scripts. The [EA board configuration](/ea/board-config/#layout)
page has the whole layout, including `board.env` and the device tree.

**Kconfig takes the last assignment**, so a board extends or overrides shared
settings simply by coming after them, and each board file stays a short list of
genuine differences rather than a full copy that drifts.

The same layering applies to the kernel: `common.fragment`, then the model's
and board's fragments, then per-feature fragments. Ordering matters and has bitten before — the
[SGX feature](/ea/graphics/) sets `CONFIG_DRM=y` while `common.fragment`
sets `# CONFIG_DRM is not set`, and it works only because feature fragments are
appended *after* the board file.

## Which Buildroot

openHC builds against **Buildroot master, pinned to a commit** (`04b8e19`, from
2026-10-26), not a release. The pin is `build/buildroot.ref`, read by
`build/fetch-buildroot.sh` for CI, Docker and native builds alike;
`sgx-module.yml` and `sgx-wpe.yml` still carry their own copy. The fetch takes
a SHA or a tag, so moving to the 2026.11 tag when it ships is a one-line change.
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

- **`build/buildroot-patches/0004` and `0005`, both wpewebkit**, sent upstream
  as one series.
  - `0004` makes wpewebkit select and depend on `libxkbcommon`, which its CMake
    requires but which it reached only indirectly, through `libwpe`.
  - `0005` adds the WPE Platform API and its headless, DRM and Wayland
    platforms as Buildroot options. The HC-800's display needs the DRM one.
  - The old `0001`, which swapped wpewebkit's `wpebackend-fdo` dependency for
    `libwpe`, is gone. The EA's i386 tree builds with `wpebackend-fdo` present,
    against the SGX EGL, and still loads the PowerVR backend. That costs about
    264 KB of unused libraries.
- **`build/buildroot-patches/0006` and `0007`**, Buildroot's own shairport-sync
  bumps to 5.6.2 and 5.6.3, taken from master until the pin passes them. 5.6.3
  has `dbus_nametag`, which audiod sets on every receiver so each has its own
  D-Bus name. It replaced openHC's two shairport-sync patches and its D-Bus
  policy file.
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
make -C board/common/firmware/io-mcu        # both images
make -C board/common/firmware/io-mcu test   # host tests
```

Buildroot builds the images into every board with the `iomcu` feature
(`host-arm-gnu-toolchain`); by hand it needs `arm-none-eabi-gcc`. The host tests
need any C compiler and run in CI. See [IO-MCU firmware](/shared/io-mcu-firmware/).

## CI

One workflow per area, so a change runs only what it touches:

| Workflow | Runs for |
|---|---|
| `docs.yml` | `site/**`; deploys Pages from main |
| `flasher.yml` | `flasher/**`; release attaches the binaries |
| `daemons.yml` | `packages/**`, `flasher/crates/core/**` |
| `io-mcu.yml` | `board/common/firmware/io-mcu/**`, `ohc-iomcu-proto.h` |
| `tools.yml` | `tools.yml`, `.github/scripts/fetch.sh`, `build/kexec-i686.sh` |
| `images.yml` | `board/**`, `packages/**`, `build/**`, `Makefile` (and `flasher/crates/core/**` on main); release, weekly schedule |

A workflow starts only when its paths change, on a pull request and on main
alike, so a docs or flasher change never starts an image build. Branch
protection requires one check, `ci-ok`, a commit status that `ci-ok.yml`
posts: pending while any area workflow started for the commit is running,
failure once one fails, success when all pass or none was needed. It is
re-evaluated each time an area workflow finishes.

The images workflow's steps are scripts in `build/`, so the same build runs on
any Linux machine (see [natively](#natively-on-linux) below):
`select-boards.sh` picks the boards a change affects (`build/select-boards.sh
origin/main` shows what your branch would build), `fetch-buildroot.sh`,
`deps.sh`, `stage-daemons.sh`, `build.sh`, `kexec-i686.sh` and `bundle.sh`. The
workflow adds only caching, the shared EA toolchain and publishing.

The images workflow runs Buildroot **directly on the runner** rather than through
the Dockerfile, deliberately: the Dockerfile keeps sources off a developer's
machine, but in CI it actively hurts, because Buildroot's `dl/` and `output/` live
in BuildKit cache mounts and cache mounts aren't exported by any GitHub cache
backend. Run it natively and `actions/cache` can hold the one thing worth holding
— the ~GB of upstream tarballs every board re-downloads, keyed so every matrix
job hits the same entry.

The EA boards share one toolchain. They are the same target (x86_64 Bonnell,
glibc, C++/wchar), so the `images-toolchain` job builds it once per run and each
EA image job uses it as an external toolchain (`OHC_EA_SDK` in `build/build.sh`).
The other boards each build their own.

A change under `board/<board>/` builds only that board; one under
`board/ea/common/` builds every EA board, and one in a model folder outside its
board folders builds that model's boards. A change to `board/common`, `build/`
or the Makefile builds everything. A change under `packages/` builds the boards
that ship the touched crate; one to `flasher/crates/core` rebuilds the boards
with the restore or update daemon, on main only. A docs-only change builds
nothing.
