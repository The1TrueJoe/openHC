---
title: Board status
description: What runs on which board today, and what each claim is based on.
sidebar:
  order: 1
---

Eight board profiles exist in the tree. They are at very different stages, and
the difference between "builds" and "booted on real silicon" is the only
distinction that matters when you are about to write something to a device.

## Where each board is

| Board | SoC | Status | What that means |
|---|---|---|---|
| **ea1-v1** | Intel CE5310 | **Proven** | Netboots Linux 7.1.8, reaches a shell, Wi-Fi and SSH up. Secure-boot fuse clear. |
| **ea3-v2** | Intel CE5310 | **Proven** | 7.1.8 via `bootlinux`, e1000 + eMMC + SSH, **persistent self-boot** from eMMC. Fuse blown; takes over via the CEFDK autoscript. |
| **ca1** | i.MX6SL | **Proven** | Boots our 7.1.8 kernel. openHC on eMMC ext4, Node, Rust dashboard, captive-portal Wi-Fi setup. |
| **ioxv1** | TI DM355 | **Proven (partial)** | Kernel boots to a login prompt, dm9000 Ethernet + SSH up, all 8 relays click, front LEDs light. Serial ports and IR still blocked on the FPGA. |
| ea1-v2 | Intel CE5310 | Builds | Boot differences unconfirmed. |
| ea1-v2-poe | Intel CE5310 | Builds | Never booted. |
| ea3-v1 | Intel CE5310 | Builds | Never booted. Its fuse state is unknown and may match the EA1's. |
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

## What is not

- **Video on the EA family beyond a 720x480 framebuffer.** CEFDK leaves a display
  pipe running and we hand that buffer to `simplefb`; nothing does a modeset.
- **Audio on the EA family.** The drivers are written and compile; four register
  fields are still inferred, so the platform driver doesn't register a PCM.
- **The BCM53125 switch under mainline DSA.** The board glue builds, but DSA has
  never attached on hardware — the second rear jack is still unusable.
- **The FPGA on the IO Extender**, which gates its four RS-232 ports and eight
  IR outputs.
- **The Zigbee NCP on the CA-1**, which answers nothing at any baud.

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
