---
title: Glossary
description: The acronyms, codenames and part numbers that recur across this site.
sidebar:
  order: 2
---

Four unrelated platforms means four unrelated vocabularies. This is the shared
index.

## Bootloaders and boot paths

**CEFDK** — Intel's Consumer Electronics Firmware Development Kit, the
bootloader on the EA family. Lives in SPI-NOR, physically separate from the
eMMC, which is what makes the EA boards hard to brick. Has a shell, two
different boot commands with different security postures, and an autorun
script facility.

**`bootlinux`** — the CEFDK shell command that loads a bare bzImage from a RAM
address. It checks the `0xAA55` and `HdrS` boot magics and **nothing else** —
no RSA, no hash, no fuse read. This one command is the entire EA takeover.

**`bootkernel`** — CEFDK's *other* boot command, and the normal-boot path. It
**does** enforce an RSA signature against a key baked into the image. A dead end;
Control4 replaced Intel's key with their own and no private key exists outside
the vendor.

**Manufacturing mode** — the state CEFDK enters when the ID button is held at
power-on. It brings up Ethernet and BOOTPs for a kernel. Every failure *after*
the cookie matches drops to the **unlocked** shell, which is how a shell is
obtained without the password.

**`C4_COOKIE`** — the literal string CEFDK looks for in DHCP option 60 of a
BOOTP reply before it will trust the offer. Ten bytes, `strcmp`'d, trailing NUL
included.

**MFH** — Master Flash Header, CEFDK's slot table in SPI-NOR. Slot types include
`kernel`, `ramdisk`, `splash` and `script`; the `script` slot is what makes the
EA3's persistent takeover possible.

**`boot.scr`** — a U-Boot script image. The CA-1's stock `bootcmd` tries to load
one from the eMMC's vfat partition *before* the stock kernel, and no such file
ships on the unit. Dropping one there takes over boot; deleting it restores stock.

**HAB** — i.MX High Assurance Boot, the CA-1's secure-boot mechanism. Read as
**Open** on the recon unit: the `SEC_CONFIG` fuse is clear and U-Boot's 8 KB
signature slot is entirely zeros.

**`run tst`** — a stock U-Boot command on the IO Extender that DHCPs, TFTPs a
kernel and boots it from RAM. Touches no flash, and is the whole bring-up loop
for that board.

## Silicon

**CE5310** — the Intel Atom SoC in the EA1 and EA3. Two cores, four threads,
i686, ~1.5 GB usable. Board codename **ninjago**.

**DM355** — TI DaVinci, ARM926EJ-S (ARMv5TE), in the IO Extender V1. Board
codename **hammer**. Mainline deleted DM355 support around v6.2.

**i.MX6SL** — Freescale i.MX6 SoloLite, a single Cortex-A9, in the CA-1. Board
codename **emmet**. The only uniprocessor board in the line.

**D525** — Intel Atom "Pineview" with an NM10 chipset, in the HC-800. A
conventional PC part. No codename, the Lego naming convention starts after
this generation.

**TM4C1231D5** — TI Tiva, Cortex-M4F, the EA family's IO microcontroller.
**64 KB flash / 24 KB SRAM** — not the 256 KB/32 KB of larger TM4C123 parts, and
getting that wrong produces a board that hard-faults before it can report
anything.

**LM3S1162** — TI Stellaris, Cortex-M3, the HC-800's and HC-250's IO
microcontroller. Same host protocol as the Tiva, different silicon and a
different image.

**EM357** — the Silicon Labs Zigbee NCP used across the line. Attached over USB
(via a CP2104 bridge) on the EA boards, and on a native SoC UART on the HC-800
and CA-1.

**ADAU1451** — Analog Devices SigmaDSP, the EA3's audio processor. Sits at I²C
address `0x38` on the CE5300's **fourth** I²C controller, a bus mainline never
creates.

**SGX545** — the PowerVR GPU inside the CE5310. Series5, which folklore calls a
dead end. It is not, because the same core shipped in Intel Cedarview and Intel
published the GPL kernel source for that.

## Project terms

**Board profile** — a directory under `board/` describing one variant. The EA
variants are composed, not copied: each lists its peripherals in
`ohc.features` and shared feature sets supply the config.

**Family base** — `board/ea-common/`, the config and patches every EA variant
shares. The HC-800 deliberately has none and never will; it is a PC and nothing
about it generalises.

**The copy window** — the ~7.0 MB ceiling on an EA kernel image. `bootlinux`
copies the protected-mode kernel to `0x100000`, and its own loader code lives at
about `0x813000`; a kernel large enough to reach that overwrites the loader
mid-copy. Every size decision on the EA boards is against this number.

**Clean room** — the rule that stock firmware is read to learn *interfaces* —
the boot flow, a wire protocol, which UART is which, and that no Control4 or
Intel code is copied into anything openHC ships.
