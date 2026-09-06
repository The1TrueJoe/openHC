---
title: Four machines, four ways in
description: The structural comparison — what kind of computer each board is, and what that implies about opening it.
sidebar:
  order: 2
---

The [matrix](/hardware/matrix/) compares boards cell by cell. This page
is the shape underneath it: four unrelated computers, four unrelated boot
chains, and four unrelated arguments about how to get code running.

## The one question that decides everything

Not "is secure boot on?" The useful question is **what is the cheapest thing
in the boot chain that will load something we control, and what does it check?**

Every board answered differently, and the answer was never the obvious one.

| Board | Cheapest way in | What it verifies | Writes flash? |
|---|---|---|---|
| **EA1** | CEFDK `bootlinux` from the mfg-mode shell | `0xAA55` + `HdrS` and nothing else | no |
| **EA3** | CEFDK `script` autorun → `bootlinux` | same — the autorun runs *before* the verifying path | SPI-NOR MFH item + a raw eMMC gap |
| **CA-1** | a `boot.scr` on the eMMC's vfat partition | nothing — `bootcmd` never calls `hab_auth_img` | one file on a FAT partition |
| **HC-800** | a third entry in GRUB's `menu.lst` | nothing anywhere in the chain | two files + one text edit |
| **IOX v1** | U-Boot `run tst` (DHCP + TFTP into RAM) | nothing | no |

Three of those five need no signature bypass because **nothing in the chain
signs anything**. The EA3 is the only board where a signature check actively
refuses our kernel, and even there the answer wasn't to defeat the check but
to take a different road that was always sitting next to it.

## What kind of computer each one is

### The EA family is an embedded set-top box

Intel CE5310, an SoC designed for televisions. Its bootloader is Intel's CEFDK
rather than U-Boot or a BIOS, and CEFDK does not read the kernel from a
filesystem. It reads it from a raw offset near the start of the eMMC, wrapped
in a 0x580-byte Intel container.

Two consequences shape everything on these boards. The kernel must be wrapped
before it can be loaded, and it must **fit in about 7 MB**, because `bootlinux`
copies it to `0x100000` and its own loader lives at roughly `0x813000`. A kernel
big enough to reach that overwrites the code doing the copying.

The 1.8 GB of RAM this board *appears* to have is also mostly not available:
CEFDK's e820 map hands Linux the first 200 MB and reserves the rest for the
Intel CE media stack that openHC never loads.

### The HC-800 is a PC

Lite-On motherboard, AMI BIOS with real DMI/SMBIOS, Atom D525, SATA SSD, GRUB
0.97. Every driver it needs — `ahci`, `r8169`, `snd_hda_intel`, `i2c_i801`,
`lpc_ich`, `iTCO_wdt`, `8250`, has been mainline for a decade, so **openHC
needs zero kernel patches for this board**. No SoC resurrection, no
de-device-tree patching, no board DTS to reconstruct.

Its boot chain is a text file. There's no container format and no size ceiling.
This is the cheapest board in the repository to bring up, and the reason it's
still unbooted is availability, not difficulty.

It's also, precisely because it's a PC, **not a family**. Nothing about it
generalises, which is why `board/hc800` is standalone with no shared base and
will never get one.

### The CA-1 is a normal ARM embedded board

Stock U-Boot 2014.04 in SPI-NOR, an eMMC with a FAT kernel partition and an ext4
root, and an SoC that mainline has supported properly for years. There are no
resurrection patches to write and no container to wrap; `imx_v6_v7_defconfig`
covers i.MX6SL and what the board needs is a DTS.

It has two quirks that took real time. Its U-Boot console is **SHA-256
password-gated**, the identical hash the EA's CEFDK uses, so there's no way
to a U-Boot prompt over serial at all. And its device tree is a lightly-edited
i.MX6SL EVK tree that still claims `compatible = "fsl,imx6sl-evk"` and carries
EVK-only devices that aren't on the board. Trust the pin groups, not the
properties.

### The IO Extender is an SoC mainline deleted

TI DaVinci DM355, ARMv5TE, dropped from mainline around v6.2. Bringing it up
meant resurrecting four files from pre-6.2 git history and forward-porting them
into frameworks that still exist because the da850 platform keeps the DaVinci
subsystem alive.

That sounds worse than it is. **Every peripheral driver the board needs is still
in 7.1** — `gpio-davinci`, `ti-aemif`, `davinci_nand`, `dm9000`, `8250`, `edma`,
`i2c-davinci`, `davinci_wdt`, because da850 uses them all. What was removed was
only the SoC glue.

It's also the only board whose IO is genuinely on-board: relays, contacts,
buttons and LEDs are plain SoC GPIO with no microcontroller in between, so
driving them needs no driver at all, just libgpiod.

## Where the IO actually lives, and why it matters

This is the split that catches people, because it's invisible from a running
system's device list.

**EA family and HC-800: a companion microcontroller owns the IO.** The IR jacks,
relays and contacts hang off a TM4C (EA) or an LM3S (HC), reached over a host
UART with a DLE/STX protocol. There are no `/sys/class` entries, no GPIO lines
and no device nodes for any of it. Reproducing it means
[implementing the wire protocol](/shared/io-mcu/).

**CA-1: there's no IO at all.** No IR, no relays, no contacts, no MCU. Its
single rear serial port is a host i.MX UART sitting behind a transceiver that
seven GPIOs reconfigure.

**IO Extender: everything is SoC GPIO**, except the four serial ports and eight
IR outputs, which live behind an FPGA on the memory bus.

So "how do I toggle a relay" has three completely different answers depending on
which black box is in front of you.

## What generalises, and what does not

The EA1 and EA3 turned out to be **the same computer**, same CE5310, same
1.5 GB, same eMMC layout, same CEFDK, byte-identical IO-MCU firmware, same board
codename. The whole difference is peripherals and one fuse. That's why EA3
support is a board profile rather than a port, and why the EA variants are
composed from feature sets rather than copied.

Nothing else in the line shares anything. Four SoCs, four bootloaders, four
device-tree situations, two IO architectures and one board with no IO at all.
The shared surface across the whole tree is the Buildroot scaffolding, the
`board.env`-driven init, and the IO-MCU protocol between the EA and HC families
— and even that protocol runs on two different Cortex-M cores that can't share
a build.
