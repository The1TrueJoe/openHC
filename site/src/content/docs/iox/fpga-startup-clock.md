---
title: The FPGA startup problem
description: DONE never rose, and it looked like a clock or pin fault. It was two NAND bugs and a driver that believed a failed load.
sidebar:
  order: 5
---

**Solved.** The symptom: the bitstream loads with INIT_B high throughout, but
DONE never releases. The version register reads garbage (`0x0202` on a blank
bus, later `0x8000`) instead of `0x0400`. Two separate faults produced it.

## 1. The config pins were wired to address lines

`M2`/`M0`/`DIN` share pins with EMIF address lines. The vendor's `c4fpga.ko`
sets `PINMUX2[0:1]` for the duration of the load and clears them afterwards.
openHC only ever held the post-load value, so DIN never reached the part. The
driver now does the same dance (see its PINMUX2 comment and the
[IO map](/iox/io-map/)).

## 2. The bitstream read off NAND was corrupt

With the pins fixed, loading a bitstream copied off NAND *by openHC* still left
DONE low. The one copied off a running vendor OS configured fine. Same size,
different md5: 1,082 bytes of the openHC copy were zeros.

**The device tree had the wrong NAND ECC.** It declared 4-bit; the flash is
written with DaVinci **1-bit** hardware ECC, three bytes per 512-byte sector at
OOB offsets 40–51, stored inverted. That was proved by recomputing those bytes
from a raw `nanddump`: every sector of the vendor kernel matches, nine of them
with a genuine single-bit flip that the ECC corrects and the data CRC then
confirms. It is exactly mainline `davinci_nand`'s 1-bit mode, so the fix is
`ti,davinci-ecc-bits = <1>`.

With 4-bit, every read came back uncorrected and the aging flash's bit flips
went straight through. JFFS2 caught one bad node in `fpga_fw.bin` and did what
it does with a node it cannot verify: dropped it and read that range as a hole.
So `cat` succeeded and returned a file with zeros in the middle.

The same mismatch explained a recurring "Bad block table not found ... written"
on every boot. The stock U-Boot also keeps an on-flash BBT in the last two
blocks, written with 1-bit ECC. Each side failed to read the other's table and
rewrote it. openHC no longer uses an on-flash BBT at all (`use-bbt` is dropped)
so a netboot never writes NAND.

## 3. The driver believed a failed load

The success check rejected only the obvious float patterns. `0x8000` with
DONE=0 passed, so the driver populated the four UARTs and the IR block on an
unconfigured part, and the box hung within seconds. DONE=0 is now a failure.

## Two hazards worth knowing

- **Nothing may touch EMIF during a load.** The PINMUX2 switch takes address
  lines away from the dm9000 and the NAND on the same bus. A network interrupt
  mid-load hangs the box with `dm9000 ... status check fail` forever. At boot
  `S12fpga` runs before networking; on a manual re-run it takes `eth0` down for
  the load and puts it back.
- **Where the bitstream comes from.** It is proprietary and never in the repo.
  `S12fpga` copies it from the unit's own **recovery rootfs** (falling back to
  the two update banks), read-only. Every copy on a stock unit has the same md5.

## What was ruled out along the way

None of these were the cause, and each was checked against a working vendor OS:
the VENC/video clock, IRQ-off gap-free clocking, the load sequence (matched to
the vendor's `c4fpga.ko`), and the PLL, PSC, PINMUX and EMIF CS1 registers
(identical). Reading NAND before the load turned out not to matter on its own.
What mattered was *what* it read.
