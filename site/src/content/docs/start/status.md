---
title: Board status
description: What runs on which board today, and what each claim is based on.
sidebar:
  order: 1
---

Eight board profiles exist in the tree. They are at very different stages, and
the difference between "builds" and "booted on real silicon" is the only
distinction that matters when you are about to write something to a device.

:::note[All eight build, and that is newer than it sounds]
Until 2026-09-10 only the hc800 actually produced an image. The other seven each
failed for a different reason — a board-name mismatch, a missing linker entry, a
64-bit atomic ARMv5 does not have, three absent `board.env` files, a `set -e`
misfire, a kernel driver mirrored onto two boards that have no such hardware, and
a 64-bit divide that needs a libgcc helper the kernel never links. Every one of
them was invisible while CI only ever built one board. The row below says
"Builds" because a run now says so, not because nothing has objected.
:::

## Where each board is

| Board | SoC | Status | What that means |
|---|---|---|---|
| **ea1-v1** | Intel CE5310 | **Proven** | Netboots Linux 7.1.8, reaches a shell, Wi-Fi and SSH up. Secure-boot fuse clear. Boot SPI-NOR is a writable `/dev/mtd0`; software return-to-stock and the hardware restore button are in place; the Zigbee NCP answers. |
| **ea3-v2** | Intel CE5310 | **Proven** | 7.1.8 via `bootlinux`, e1000 + eMMC + SSH, **persistent self-boot** from eMMC. Fuse blown; takes over via the CEFDK autoscript. |
| **ca1** | i.MX6SL | **Proven** | Boots our 7.1.8 kernel. openHC on eMMC ext4, Node, Rust dashboard, captive-portal Wi-Fi setup. |
| **ioxv1** | TI DM355 | **Proven** | Netboots 7.1.8 into RAM (no flash install yet). The FPGA loads itself at boot from the unit's own NAND, giving four RS-232 ports (verified both ways against a PC) and eight IR outputs (all eight verified against a GC-IRL learner at 38 kHz). Relays, contacts, the tri-colour status LED, data/link LEDs, iod and the web UI all work. |
| ea1-v2 | Intel CE5310 | Builds | Never booted; boot differences unconfirmed. `board.env` inherited from the ea1-v1, not read off this variant. |
| ea1-v2-poe | Intel CE5310 | Builds | Never booted. `board.env` is the EA1's IO with the EA3's networking, per its defconfig — unverified. |
| ea3-v1 | Intel CE5310 | Builds | Never booted. Its fuse state is unknown and may match the EA1's. `board.env` is the ea3-v2's plus the radio it kept. |
| **hc800** | Atom D525 | **Proven** | 7.1.8 booted, then **persistently installed** (GRUB entry on the spare kernel partition, boot-once via `savedefault`). Relays, contacts, six named IR devices, all six front LEDs, web UI, iod and sysmond. No kernel patches at all — the cheapest board here to bring up, as predicted. |

EA5 and HC-250 are not supported. The IO-MCU firmware tree has room for the
HC-250's Stellaris part, and the decoded MCU profile table already describes the
EA5, but neither board has been in hand.

## What is genuinely finished

- **Booting an unsigned, self-built kernel** on the EA family, on the CA-1, and
  on the DM355 — three completely different bootloaders, three different
  bypasses, none of them requiring a signature.
- **Persistent installs that survive a power cycle** on the EA3, the CA-1 and
  the HC-800, with the stock image left in place as a one-button recovery. The
  HC-800's is deliberately a **boot-once**: GRUB hands the default back to
  Control4 as openHC starts, so a panic, a watchdog reset or a power cut always
  returns to a system that answers SSH. See [Working without
  serial](/shared/headless/).
- **The IO-MCU wire protocol**, end to end: bring-up out of the bootloader,
  the DLE/STX framing, IR transmit confirmed emitting 38 kHz on a live jack,
  IR capture decoding a real NEC remote, and contacts measured in both
  directions.
- **GLES2 on the PowerVR SGX545**, which the prevailing wisdom said was
  impossible. Shaders compile, a triangle rasterises, and it was confirmed on a
  panel.
- **The DM355 SoC resurrection**, four files pulled back from pre-6.2 mainline
  history and forward-ported until a 7.1.8 kernel boots on ARMv5 silicon that
  upstream deleted.
- **A writable boot SPI-NOR on the EA family.** The CE5300's dedicated boot-flash
  controller (PCI `8086:08a0`, a 16 MB S25FL127S, 256-byte pages) is driven by the
  in-tree `spi-ea-ce5xx`, so the chip comes up as a writable `/dev/mtd0`. Confirmed
  on an EA1: `/proc/mtd` shows a 16 MB `mtd0` on `spi0.0`. This is what makes an
  in-place install *and* a software return-to-stock possible without serial.
- **Software return-to-stock on the EA**, openHC's own, needing no serial and no
  button. The install makes exactly one boot change — a single `script` item
  appended to the SHA-256-protected MFH item-table in SPI-NOR — so reverting is
  removing that one item, a byte-for-byte inverse the installer's code is
  unit-tested against the known-stock backup. The `mtd0` write path is **proven**:
  it rewrote the MFH on a corrupted live EA1 and read it back verified. The full
  `stock` round-trip all the way back to Control4 has **not** yet been run end to
  end on hardware. See [recovery](/shared/recovery/).
- **A hardware return-to-stock button on the EA.** Holding the front ID button —
  a bare SoC GPIO the board driver deliberately leaves unclaimed — for ten seconds
  runs the software restore. It is fail-safe: it arms only after one clean
  "released" reading, so a wrong line or polarity stays inert rather than wiping a
  box at boot. The fail-safe behaviour was confirmed on an EA1.
- **The EA1 Zigbee NCP**, which now answers. The EHCI `has_hostpc` fix for the
  CE5300 TDI USB core lets the CP2104 bridge enumerate and the EM357 replies: a
  bare ASH reset returns `RSTACK` (0xc1) at 115200 8N1, measured on hardware.

## What is not

- **Video on the EA family beyond a 720x480 framebuffer.** CEFDK leaves a display
  pipe running and we hand that buffer to `simplefb`; nothing does a modeset.
- **Audio on the EA family.** The drivers are written and compile; four register
  fields are still inferred, so the platform driver doesn't register a PCM.
- **The BCM53125 switch under mainline DSA.** The board glue builds, but DSA has
  never attached on hardware — the second rear jack is still unusable.
- **A flash install on the IO Extender.** It runs from RAM over TFTP.
- **The Zigbee NCP on the CA-1**, which answers nothing at any baud. (The EA1's
  NCP now does answer; this caveat is CA-1-specific.)
- **The full `stock` return-to-Control4 round-trip on the EA, end to end.** The
  `mtd0` write path that it rests on is proven, but reverting the MFH *and*
  recovery-kexec'ing p2 to reimage p1 has not yet been run through on hardware.
- **The return-to-stock button on the EA3.** The software path ships there too, but
  that board's GPIO line and polarity are not hardware-confirmed yet, so the
  fail-safe keeps the watcher inert until they are.
- **The HC-800 software factory-restore.** `ohc-flash factory-restore` flips the
  GRUB default to Control4's own recovery entry; it is code-complete and
  read-back-verifies, but has not been run against real HC-800 hardware.
- **The EA1 RAM reclaim.** EA1-v1 still boots on the ~176 MB CEFDK hands over,
  while ~1.8 GB sits "device reserved". The `memmap=exactmap` line that gives the
  EA3 its 1.7 GB is now written for the EA1 too — its e820 split and the fact that
  the carve-out is real, idle DRAM are *measured* on an EA1, identical to the EA3 —
  but it has not yet been booted with on an EA1, so it stays unproven until it is.

## How to read a claim on this site

Three phrases are used deliberately and mean different things:

**Measured / confirmed on hardware.** Someone read it off a live unit or watched
it happen. These are the load-bearing facts.

**Decoded / derived.** Extracted from a firmware image, a kernel binary or
published source, with the reasoning shown. Usually reliable, occasionally
wrong in a way that costs a day, the [IO-MCU profile table](/shared/io-mcu/)
has two corrections written into it for exactly this reason.

**Inferred / unverified.** A reasonable guess nobody has checked. Never act on
one of these irreversibly.

The single most expensive mistake in this project was a *confident* wrong
reading: the secure-boot fuse was measured at the wrong register offset, and
the conclusion "signature checking isn't enforced" held for weeks before an
EA3 refused to boot and proved it board-dependent. That correction is preserved
in full on the [GPL source](/shared/gpl-source/) page rather than quietly
edited away.
