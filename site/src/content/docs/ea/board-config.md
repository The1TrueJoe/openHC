---
title: Board configuration
description: Why the EA defconfigs, feature sets and board.env files say what they say, and what is still unverified on each board.
sidebar:
  order: 13
---

The comments in the EA config files are kept to one line. This page holds the
background: why each setting is there, what has been measured, and what is
still open on each board.

The files:

| File | Role |
|---|---|
| `board/ea/common/common_defconfig` | EA family base. Not standalone; `build/build.sh` composes it with the rest |
| `board/ea/common/external-toolchain.defconfig` | Optional switch to a pre-built toolchain (`OHC_EA_SDK`) |
| `board/ea/common/features/<name>/defconfig` | One feature's packages, enabled from `ohc.features` |
| `board/ea/<board>/<board>_defconfig` | Hostname and `/etc/issue`, nothing else |
| `board/ea/<board>/rootfs-overlay/opt/ohc/board.env` | Runtime board description read by init scripts and daemons |

The composition order and the "last assignment wins" rule are on the
[build page](/build/#how-the-configuration-is-assembled). Only put something in
`common_defconfig` if it is true for the whole family; a board overrides it by
setting it again later.

## Family base

### Architecture: x86_64, Bonnell tuning

Every EA is the same Intel Atom CE5310, which reports `lm`. CEFDK's
`bootlinux` starting a 64-bit kernel was proven on hardware on 2026-08-31.

The move off i386 was for Home Assistant: orjson and cryptography have never
published i686 wheels, and HA pins exact versions. It also removed the 32-bit
lowmem split (about 934 MB of the 1.7 GB had been highmem).

The cost was that the SGX545 userspace is 32-bit Intel/Imagination blobs, so 3D
has to run as i386 under the 64-bit kernel's compat path. That was an open
question when the switch was made; it now runs, see [graphics](/ea/graphics/).

### Kernel build

| Setting | Why |
|---|---|
| `BR2_LINUX_KERNEL_NEEDS_HOST_LIBELF` | x86_64 builds objtool, which includes `<gelf.h>`. Without host libelf the kernel build fails with `tools/objtool/include/objtool/elf.h:10:10: fatal error: gelf.h`, which names a host header, not a missing setting |
| `BR2_OHC_EA_KERNEL_DRIVERS` | Mirrors `board/ea/common/kernel/{drivers,sound}` into the kernel tree through `OHC_KERNEL_MIRROR_HOOK`. Opt-in, and `common_defconfig` is the only file that sets it. If an EA kernel is missing `gpio-intelce`, `pwm-ce5300` or the ADAU1451 ASoC stack, check this first |
| `BR2_LINUX_KERNEL_BZIMAGE` | The bzImage has to fit CEFDK `bootlinux`'s ~7 MB copy window, which is what the trim in `linux/common.fragment` is for. See [the copy window](/ea/boot-chain/#the-copy-window) |

### Root filesystem as an external initrd

The cpio is gzipped and loaded as a separate initrd, not embedded in the
bzImage. CEFDK's `bootlinux` copies the kernel to `0x100000` while its own
loader sits at about `0x812f07`, so a large embedded image overwrites the loader
mid-copy and crashes on return. The initrd goes in high RAM instead. Detail on
the [boot chain](/ea/boot-chain/) page.

### Tools on target

- **`pciutils`**: on-target debugging.
- **`mtd`**: `flash_erase`, `flashcp`, `mtd_debug`. These are how openHC backs
  up, rewrites and restores the CEFDK MFH and autoscript once `spi-ea-ce5xx`
  exposes the boot SPI-NOR as `/dev/mtd0`.
- **`kexec` with zlib**: the autoscript can only load a kernel that fits its
  fixed read size, so the full kernel (I²C, fan, the SPI-NOR driver) is staged
  on p1 and started by `/etc/init.d/S00kexec`. The HC-800 updates itself the
  same way. zlib because the bzImage is gzip-compressed.

### ccache

Every EA board is the same x86_64 glibc target and compiles the same heavy
userspace: glibc, ICU, harfbuzz, libstdc++ and above all WPE WebKit (about
1.1 GB per job, the dominant cost). ccache wraps `HOSTCC` and the cross
compiler, so an object built for ea1-v2 is a hit for ea3-v2 and for the next
run, the toolchain included.

CI points `BR2_CCACHE_DIR` at a cached directory (`.github/workflows/ci.yml`)
and the whole EA family shares one key, so the first board fills it. Locally,
with no directory set, it uses Buildroot's default and starts empty. The cap,
`--max-size=3G`, keeps one family's cache from pushing the others out of
GitHub's 10 GB repository cache budget; ccache evicts its own LRU objects past
it.

### C library and optimisation

**glibc, not the fleet's musl default.** The PowerVR DDK userspace
(`packages/sgx545-um`) is glibc-linked and cannot load on musl. gcompat is in no
Buildroot release, and a private glibc under `/opt` would mean a loader wrapper
forever. Size does not argue against it: the image is 512 MB with about 7 MB
used. What is tight is the bzImage, which the C library does not touch.

**`-O2`, not the fleet's `-Os`.** There is room, and glibc 2.42 did not link at
`-Os` on x86_64 without a backport (BZ 33367, GCC leaving `modf`'s `trunc()` as
a libcall; fixed upstream, see [Which Buildroot](/build/#which-buildroot)). The
kernel has its own `CONFIG_CC_OPTIMIZE_FOR_SIZE`, so the bzImage is unaffected.

**C++ and wide characters** (`BR2_TOOLCHAIN_BUILDROOT_CXX`,
`BR2_INSTALL_LIBSTDCPP`, `BR2_USE_WCHAR`) are required by WPE WebKit, ICU and
harfbuzz. Without them `wpewebkit` is unselectable and kconfig drops it
silently: the build succeeds and the engine is absent.

### Font

`BR2_PACKAGE_DEJAVU` puts a real font in `/usr/share/fonts`. The on-panel WPE
WebKit (the i386 sysroot in `/opt/sgx-i386-wpe`) ships fontconfig but no font
files, and its `<dir>/usr/share/fonts</dir>` resolves to this image's
directory. Without a font every glyph renders invisibly and the dashboard looks
blank (its light background with no text). DejaVu is fontconfig's canonical
generic, so the UI's `system-ui -> sans-serif` stack lands on DejaVu Sans. Only
the display path uses it.

### Patches and post-image

`BR2_GLOBAL_PATCH_DIR` and `BR2_ROOTFS_POST_IMAGE_SCRIPT` both point into
`board/ea/common`, so they are family settings, not per-board. The patch list
starts with `board/common/patches`, shared with the HC-800: `0015` (the
built-in device tree) and the shairport-sync patches of the audio feature.

## External toolchain

`external-toolchain.defconfig` is not part of the normal composition.
`build/build.sh` appends it, plus a generated `BR2_TOOLCHAIN_EXTERNAL_PATH`
line, only when `OHC_EA_SDK` points at a pre-built EA toolchain. Without it the
boards build their own internal glibc toolchain from `common_defconfig`.

All EA boards are the same target, so the ~15-minute host gcc build is
identical for each. CI builds it once in a `toolchain` job, packages
`output/host`, and every EA image job uses that as an external toolchain. The
job that builds the SDK sets `OHC_TOOLCHAIN_ONLY` and keeps the internal one.

The file is appended right after the family base, so its `BR2_TOOLCHAIN_EXTERNAL`
wins the toolchain choice. The capability flags must match what the SDK
actually is, or Buildroot's toolchain check fails at configure time. They must
track `board/ea/common/common_defconfig` and `board/common/common_defconfig`:

| Flag | Source |
|---|---|
| `GCC_15` | Buildroot's default gcc for x86_64 (15.3.0) |
| `HEADERS_7_1` | `board/common` sets `BR2_PACKAGE_HOST_LINUX_HEADERS_CUSTOM_7_1` |
| `CUSTOM_GLIBC` | the family runs glibc for the DDK blobs |
| `CXX` | WPE WebKit and ICU |
| `INET_RPC` off | see below |

Two traps:

- **`BR2_TOOLCHAIN_EXTERNAL_CXX` fails silently.** Every other mismatch fails
  loudly; omitting this one drops every C++ package (ICU, harfbuzz, WPE WebKit)
  without an error.
- **RPC must be off.** For a glibc external toolchain Buildroot assumes
  wchar, locale, threads, NPTL and SSP, and those knobs are hidden. RPC defaults
  on, but glibc 2.26+ dropped SunRPC (it moved to libtirpc), so the check aborts
  with "RPC support not available in C library, please disable
  BR2_TOOLCHAIN_EXTERNAL_INET_RPC". The internal toolchain had no native RPC
  either, so nothing in the image changes.

## Feature packages

Each feature's kernel half is the `linux.fragment` beside its `defconfig`.
Which board gets which feature is in `board/ea/common/ohc.features` (every EA)
and `board/ea/<board>/ohc.features`.

| Feature | Packages | Notes |
|---|---|---|
| `audio` | alsa-lib, `aplay`, `amixer`, `speaker-test`, Avahi, librespot | `speaker-test` tells "no sound" from "no card". AirPlay 2 (shairport-sync + nqptp) comes from the shared `board/common/features/audio`. Avahi is the full responder AirPlay announces through; openHC's own `_openhc._tcp` responder is too small for other AirPlay implementations. D-Bus comes from `board/common/common_defconfig`. librespot is openHC's own Rust package (`packages/librespot`). See [audio](/ea/audio/#airplay-and-spotify-connect) |
| `audio-dsp` | `adau1451-fw` | The EA3's ADAU1451 parameter file, built from `board/ea/common/firmware/dsp/adau1451/` and loaded by the codec driver from `/lib/firmware/openhc/` |
| `emmc` | ext4 image, e2fsprogs + resize2fs | 512 MB base image written to p1, grown online to fill the partition by `S03growfs` on the first boot that finds it short. A build that needs more in the image itself (Node, Python, HA) raises the size in its own layer |
| `restore` | kexec | The `restore` package edits the MFH over `/dev/mtd0` and, for a full restore, kexecs p2's recovery kernel. MTD is already in `common.fragment`. See [recovery](/shared/recovery/#openhcs-own-software-return-to-stock) |
| `sgx` | `sgx545-srvinit` | The kernel driver (PCI `01:02.0`, `8086:089b`, ported from Intel's Cedarview GPL DDK 1.7) is the `kernel/` of the `board/ea/common/sgx545-ce` submodule, built as `CONFIG_SGX545_CE=m`, giving `/dev/dri/card0`. srvinit is the open GPU bring-up, run once by `S60sgx`. See [graphics](/ea/graphics/#bringing-the-gpu-up-at-boot) |
| `switch` | iproute2, bridge-utils | switchd needs iproute2: `bridge vlan` is how 802.1Q filtering is offloaded to the b53, and `brctl` cannot filter VLANs at all. bridge-utils is only kept for habit and scripts |
| `wifi` | wpa_supplicant (nl80211, CLI, passphrase), iw | Boards with the ath9k radio |
| `wpe` | `sgx545-um`, wpewebkit, `wpebackend-pvr`, `webview` | The proprietary half of 3D: DDK 1.7 EGL + GLES2, fetched at build, not vendored. It must be the same DDK version as the driver, which refuses a mismatch on connect. Buildroot also builds wpebackend-fdo and Wayland for wpewebkit; nothing uses them. `wpebackend-pvr` is the libwpe backend that reaches the display; `webview` puts a page on screen (WebKit is built without MiniBrowser). Split from `sgx` so the open driver and the blobs are separable |

**`BR2_PACKAGE_SHAIRPORT_SYNC_ALSA` does not exist.** shairport-sync always
builds `--with-alsa`. A line setting it would be discarded by kconfig without a
word, the same kind of silent miss as `CONFIG_PWM_CE5300` or `NET_DSA_B53`.

## board.env

`board.env` is sourced by `/opt/ohc/bin/ohc` and the init scripts, and iod and
audiod parse it line by line as `KEY=value` with anything after `#` stripped.
Shell assignments only, one per line.

### Identity

| Key | Meaning |
|---|---|
| `OHC_MODEL` | 1, 3 or 5. The ea1-v2-poe is 1 despite carrying the EA3's switch |
| `OHC_ID_IFACE` | The interface whose MAC names the box; always the wired one |
| `OHC_MDNS_IFACE` | Interface order for the mDNS identity; `wlan0` only where there is Wi-Fi |
| `OHC_TIMEZONE` | Host timezone; change it with `ohc-tz <Zone>` |

### IO

`OHC_IO_BACKEND` is the only thing ohc-iod branches on:

| Value | Meaning |
|---|---|
| `mcu` | Relays, contacts and IR are behind a microcontroller on a serial port, spoken to with DLE/STX |
| `gpio` | They are native SoC GPIO lines |
| `none` | No local IO |

Every EA is `mcu` with `OHC_IO_MCU_PART=tm4c1231d5`. What sits behind the part
(relays, contacts, IR jacks, the front blaster and receiver, the MCU-routed
serial ports) and its UART and line rate are not in `board.env`. They are in the
board's device tree (`&io_mcu` in `<board>.dts`); the kernel binds the part from
it at boot and iod reads the counts back from `/sys/class/ohc/iomcu`, so there
is one description that cannot disagree with itself. The combo jacks are the
MCU-routed UARTs: a jack is IR or serial, never both. `OHC_SERIALS` is empty on
all EA boards. See [the IO-MCU](/shared/io-mcu/) and
[IO naming](/shared/io-naming/).

### Zigbee

`OHC_RADIOS` lists home-automation radios as `type:dev`; the UI shows a panel
for each. Every EA has only Zigbee (no Z-Wave), from the shared `zigbee` feature
(node + cp210x). Boards without Wi-Fi still have it.

The radio is an EM357 NCP behind a CP2104 USB bridge (`/dev/ttyUSB0`), not a
host UART, at 115200 8N1 with no RTS/CTS. Unlike the CA-1, which resets the NCP
on DTR, the EA holds it on `zigbee_reset` (gpio29), which the board device tree
hogs high (released) at boot. The chip runs its application from power-on and
answers EZSP/ASH directly, so opening the port does not drop it into the
bootloader. On an EA1 a bare ASH `RST` returns `RSTACK` (`0xc1`). See
[EA-1: Zigbee](/ea/ea1/#zigbee-the-bootloader-trap).

| Key | Meaning |
|---|---|
| `OHC_ZIGBEE_TTY`, `OHC_ZIGBEE_BAUD` | `/dev/ttyUSB0`, `115200` |
| `OHC_ZIGBEE_TCP_PORT` | `6638`. The NCP ships on EmberZNet EZSP v4; the zigbee daemon reflashes it at boot to the feature's 6.7.8 image (EZSP v8, `packages/zigbee/src/ncp.rs`). zigpy's bellows speaks both. The daemon holds the port and relays it over TCP, and `S91ohczigbee` advertises `_zigbee-coordinator._tcp` (ser2net style) so ZHA attaches as `socket://<box>:6638` |
| `OHC_ZIGBEE_RADIO_TYPE` | `ezsp` |
| `OHC_ZIGBEE_STOCK_ARCHIVE` | `/dev/mmcblk0p2:/recfs.tar.xz`. A return to stock first puts Control4's own NCP image back, unpacked from p2's factory rootfs (`zigbee --restore-stock`). Same member path as the HC-800's; checked on an EA1 on 2026-10-26 |

### Restore button

Holding the front ID button for `OHC_RESTORE_BUTTON_HOLD` seconds (10) and
releasing it runs `restore stock`; `OHC_RESTORE_BUTTON_LED` (`warn::red`) blinks
while it is held. The line, polarity and debounce are a `gpio-keys-polled` node
in `ea.dtsi`, so `S96ohc-restore-button` watches `/dev/ohc/input/id-button` and
`board.env` names no line. Only a press the watcher saw both start and end
counts, so a stuck button, or a wrong polarity, stays inert instead of wiping
the box at boot.

On a fuse-blown EA1 the native CEFDK button recovery does not rewrite `mtd0`, so
this is the dependable path there. The EA3 also has the recessed CEFDK recovery
button; this is the software equivalent.

Active-low is the family default and is confirmed on an EA1. **It is not
confirmed on EA3 hardware.** To check, watch the node while pressing and
releasing:

```sh
od -An -tx1 /dev/ohc/input/id-button   # prints events on press and release
```

Full detail on [recovery](/shared/recovery/#openhcs-own-software-return-to-stock).

### Audio

The scheme is the HC-800's: audiod runs Spotify Connect and AirPlay endpoints
onto named outputs, each output's level is an ALSA softvol, and it writes
`/etc/asound.conf`, whose `default` is the first output so any other ALSA
program plays there too. `OHC_LIBRARY` is the music-library player (mpd over the
drives storaged mounts on `/media`). See [audio](/ea/audio/).

| Board | Outputs | Rate |
|---|---|---|
| EA1 (all three) | `hdmi` only | 48000, the HDMI clock's one rate |
| EA3 (both) | `analog`, then `hdmi` | 48000, the DSP program's one rate (HDMI matches it) |
| EA-5 | see [EA-5](/ea/ea5/) | 48000 |

**HDMI** (`hw:CARD=HDMI,DEV=0|HDMI|format=S32_LE`) is the HDMI transmitter's own
audio DMA (`ce5300-hdmi`). Its FIFO takes 32-bit words, hence `format=`, and its
clock gives 48 kHz only. Playing switches the link from DVI to HDMI mode if
CEFDK left it in DVI. See [HDMI out](/ea/audio/#hdmi-out-every-ea).

**EA3 analog** is CE5300 I2S1 → ADAU1451 SigmaDSP → AK4621. Tone is done by the
DSP, not audiod's software stage: `dsp=` names the profile describing the DSP's
knobs (`adau1451-ea3-openhc.json`, the same file the DSP firmware is built from
in `board/ea/common/firmware/dsp/adau1451/`), and audiod sets them as the
card's controls. It boots silent and stays muted while idle: the DSP volume
starts at 0 until audiod restores the saved level and tone, and the DAC unmutes
only while a stream plays.

The analog card is `openhcea`, the board device tree's simple-audio-card. Until
the CE5300 I²S PCM is finished (`ce5300_i2s.enable_pcm`) it has the DSP
controls but no playable PCM, so the analog endpoints come up and cannot play
yet. See [what is still unproven](/ea/audio/#what-is-still-unproven).

### Switch ports

`OHC_DSA_PORTS` lists the BCM53125's DSA port netdevs (`lan1 lan2` on the EA3s
and the ea1-v2-poe). `S39ea-switch`, the shared `S40net` and `S87ohcswitch` use
it; it is unset on a board with one plain MAC. DSA only attaches with
`ohc.switch=1` on the kernel command line. See [Ethernet and the
switch](/ea/switch/).

### RAM reclaim: `OHC_KEXEC_CMDLINE_EXTRA`

CEFDK withholds about 1.8 GB as a media carve-out openHC never uses, leaving
an EA1 about 176 MB. The `memmap=exactmap` line that gives it back, entry by
entry, is on [kernel configuration](/ea/kernel-config/#ram-reclaim-memmapexactmap).
**Do not widen it past `0x70000000`**: a `memmap` past the end of real DRAM is
not recoverable remotely.

The EA1 boards put it in `board.env` because they boot through the `S00kexec`
bootstrap, whose `--command-line` replaces the staged kernel's `CONFIG_CMDLINE`,
so a reclaim in the board fragment would never take effect. The EA3s boot
their kernel directly and carry it in `CONFIG_CMDLINE`; their `board.env` has
no such key.

Proven on the ea1-v1 at 10.0.0.127: `MemTotal` went from 180384 kB to
1783332 kB (about 1.7 GB), ACPI clean, `mtd0` intact. The ea1-v2 and ea1-v2-poe
carry the same line, unverified on their hardware. A bad line there is
recoverable: power-cycle and the bootstrap kernel comes back.

## Per-board

| Board | Hostname | Features beyond the family | Status |
|---|---|---|---|
| ea1-v1 | `openea1` | `wifi` | Proven. Secure-boot fuse clear, boots an unsigned kernel; Wi-Fi is the reachability path |
| ea1-v2 | `openea1` | `wifi` | Unverified on hardware |
| ea1-v2-poe | `openea1` | `switch` | Unverified on hardware, fuse included |
| ea3-v1 | `openea3` | `wifi switch audio-dsp` | Unverified on hardware |
| ea3-v2 | `openea3` | `switch audio-dsp` | Proven. Fuse blown; takeover via autoscript |
| ea5 | `openea5` | `wifi switch audio-fpga` | See [EA-5](/ea/ea5/#bring-up-checklist) |

### ea1-v1

The reference EA1. Every `board.env` value was read off this board.

### ea1-v2

Close to the ea1-v1: Wi-Fi, no managed switch. Its `board.env` is inherited from
the ea1-v1, not read off a v2. Boot is where the two might differ, and boot is
not described by `board.env`, so copying the v1's IO and networking is a
reasonable reading, but every value should be confirmed the first time a v2 is
on the bench. The IR and UART counts (now in the device tree) are the ones a
variant is most likely to change. Before the file existed the board shipped
with no `board.env`, and every init script fell back to unwritten defaults.

Open: whether its secure-boot fuse is blown like the ea3-v2's. Netboot it first,
which writes nothing. If the eMMC path rejects an unsigned kernel it needs the
same autoscript takeover as the ea3-v2. Confirm `MemTotal` after the first
kexec boot.

### ea1-v2-poe

A deliberate mix of two siblings. It drops the Wi-Fi radio and gains the
BCM53125 switch, so it is wired-only:

| Part | From |
|---|---|
| Networking | ea3-v2: DSA switch ports, no Wi-Fi |
| IO | ea1-v1: it is still an EA1, 4 IR out, no relays or contacts |

Open: the fuse, the DSA port names (`OHC_DSA_PORTS` must match what b53
registers on this board, and nobody has watched it come up), Zigbee, and the
RAM reclaim.

### ea3-v1

The ea3-v2 plus the Wi-Fi radio the v2 dropped. The only `board.env` line that
differs is `OHC_MDNS_IFACE`.

Open: the fuse. The ea3-v2's is blown and the ea1-v1's is clear. Netboot first;
if the fuse is clear it can be flashed the plain way with no autoscript. See
[secure boot](/ea/secure-boot/).

### ea3-v2

Wired-only (`e1000`), BCM53125 switch on SPI via DSA, ADAU1451 DSP analog
audio. The fuse is blown, so the eMMC path rejects an unsigned kernel. openHC
takes over through the CEFDK `script` + `bootlinux` autoscript: its kernel sits
in the raw eMMC gap and the signed stock kernel stays as the `script off`
recovery. See [EA-3](/ea/ea3/), [secure boot](/ea/secure-boot/) and
`tools/ohc-ea-takeover.py`.

### First-boot checks

Each "unverified" above comes down to one of these on a running box:

| Check | Command | Expect |
|---|---|---|
| Zigbee radio (ea1-v2-poe, both EA3s) | `ls /dev/ttyUSB0`, then the `S91ohczigbee` log | The node exists and the log shows `RSTACK` |
| Restore button polarity (EA3) | `od -An -tx1 /dev/ohc/input/id-button` | Events on press and on release |
| RAM reclaim (ea1-v2, ea1-v2-poe) | `grep MemTotal /proc/meminfo` | About 1.7 GB |
| Switch ports (ea1-v2-poe) | boot with `ohc.switch=1`, `ip link` | `lan1` and `lan2` |
