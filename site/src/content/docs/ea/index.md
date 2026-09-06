---
title: The EA family
description: Intel CE5310 controllers — what they share, how they differ, and how the tree is organised around that.
sidebar:
  order: 1
  label: Family overview
---

EA-1 and EA-3 are **the same computer**. Same Intel Atom CE5310, same ~1.5 GB of
RAM, same 7.6 GB eMMC with the same partition layout, same CEFDK build, same
board codename `ninjago`, and byte-identical IO-MCU firmware. The kernel version
string on both is `3.12.74 #8-140-ninjago.1`.

The whole difference is which peripherals are populated, and one fuse.

That's why EA3 support in this tree is a **board profile rather than a port**,
and why the variants are composed rather than copied.

## The variants

| Board | Wi-Fi | Switch | PoE | Secure boot | Status |
|---|---|---|---|---|---|
| **ea1-v1** | ath9k | — | — | **clear** | **proven** — netboots, shell, Wi-Fi + SSH |
| **ea3-v2** | — | BCM53125 | yes | **blown** | **proven** — persistent self-boot from eMMC |
| ea1-v2 | ath9k | — | — | ? | builds; boot differences unconfirmed |
| ea1-v2-poe | — | BCM53125 | yes | ? | builds, never booted |
| ea3-v1 | ath9k | BCM53125 | yes | ? | builds, never booted |

Each board lists what it has in `board/<board>/ohc.features` — `wifi`, `emmc`,
`switch`, `audio`, `sgx` — and shared feature sets under
`board/ea-common/features/` supply the configuration. Buildroot has no include
mechanism for defconfigs, so the build script concatenates them: a common file
for every board, then the EA family base, then the board's feature sets, then the
board's own file. Kconfig takes the last assignment, so a board extends or
overrides shared settings simply by coming later, and each board file stays a
short list of genuine differences.

## The identity straps

Seven GPIO lines, `board_id0..6`, aren't an opaque board number. They split
into the two values the kernel exports:

```
board_id0..3  ->  revision   1,0,0,1 = 9   ( = /proc/c4board/revision )
board_id4..6  ->  type       0,1,0   = 2   ( = /proc/c4board/type     )
```

**Type 1 is an EA1, type 2 is an EA3**, with the low nibble carrying the PCB
revision. A board profile can be selected from these pins on the host without
parsing `/proc/c4board` at all.

## Two differences that actually shape the build

### Secure boot

The EA3 board v2's CEFDK signature fuse is **blown**, so the eMMC normal-boot
path rejects unsigned kernels. The EA1's is clear.

It takes over anyway, because the shell's `bootlinux` command doesn't verify and
CEFDK's `script` autorun runs *before* the verifying path. See
[secure boot and the takeover](/ea/secure-boot/).

The EA1 needs none of this, its normal path boots unsigned already.

### Peripherals

**The EA3 has no Wi-Fi at all.** The EA1 uses ath9k as its reachability path; the
EA3 comes up on wired Ethernet with mainline `e1000`.

**The EA3 has a BCM53125 managed switch on SPI** behind the MAC, which the EA1
does not. The e1000 MAC doesn't talk to a PHY. It runs in "internal fake phy"
mode against a fixed link, with the switch behind it. See
[Ethernet and the switch](/ea/switch/).

## What is common, and where it lives

`board/ea-common/` carries everything the family shares:

```
ea-common_defconfig       the EA half of the config
features/                 composable feature sets: wifi, emmc, switch, audio, sgx
linux/                    common.fragment + per-feature fragments
patches/linux/            i2c-pxa, pwm-ce5300, e1000 fake-phy,
                          gpio-intelce (CE5300 128-line GPIO),
                          spi-ea-b53-board (BCM53125 port map)
post-image.sh             wraps the bzImage in the CEFDK container
firmware/io-mcu/          the clean-room TM4C firmware
drivers/                  leds-ea-board, the B53 board glue, ce5300-fb
sound/                    the CE5300 I2S + ADAU1451 drivers
```

Two of those patches exist because mainline is *almost* right for this SoC and
the gap is silent rather than loud:

- **`gpio-intelce`** replaces mainline's `gpio-sodaville`, which exposes only 12
  lines for this controller. Three of the EA3's six LEDs sit above that, which is
  exactly why the front panel was unreachable on a mainline kernel.
- **`i2c-pxa-pci`** hardcodes three I²C controllers for this platform. The CE5300
  has four, and the audio codec is on the fourth. See
  [audio](/ea/audio/).

## Subsystem pages

- [Boot chain](/ea/boot-chain/) — SPI-NOR to CEFDK to a kernel in a raw
  eMMC container, and the 7 MB copy window.
- [Bootloader access](/ea/bootloader-access/) — the `C4_COOKIE` protocol
  and the unlocked shell. The core how-to.
- [Secure boot](/ea/secure-boot/) — the fuse, and the autoscript takeover.
- [EA-1 recon](/ea/ea1/) and [EA-3 recon](/ea/ea3/) — live pulls.
- [Ethernet and the BCM53125](/ea/switch/) — including a measured port map.
- [Audio](/ea/audio/) and the
  [CE5300 register map](/ea/audio-regmap/).
- [Graphics](/ea/graphics/), the SGX545, which works.
- [HDMI CEC](/ea/hdmi-cec/), and why it isn't where anyone would look.
