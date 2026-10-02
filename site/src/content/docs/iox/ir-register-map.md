---
title: IR block register map
description: The FPGA IR-output registers, recovered by disassembling the vendor c4irout.ko since the source was never published.
sidebar:
  order: 5
---

`c4irout.c` was the one file missing from Control4's GPL drop, so the IR block's
register layout had to come from the compiled driver. `c4irout.ko` was pulled
off a running unit (`/mnt/jffs2/modules/.../c4davinci/c4irout.ko`, ELF with
symbols) and disassembled. This is what it does.

## Two engines, either one drives any jack

The IR output is two identical engines ("accelerators") in the FPGA, at
`0x04000220` and `0x04000230`. `c4irout_config` assigns one per minor number.
Which jacks an engine drives is its output-enable mask, so either can drive
any jack. The module's debug strings name every register `c4irout_setup`
writes, which pins the layout:

| Offset | Register | Notes |
|---|---|---|
| +0x00 | OE | output enable, bit n = jack n+1 |
| +0x02 | carrier period | 50 MHz clocks: Hz = 50e6 / period |
| +0x04 | CONTROL | bit 15 GO, 14 infinite, 13 enable, 4–12 FIFO watermark, 0 reset |
| +0x06 | repeat count | `count << 9` |
| +0x08 | FIFO | write-only pulse/space stream |
| +0x0a | repeat start | |
| +0x0c | repeat end | written as `end - 1` |
| `0x3e` | inverted / no-carrier | shared by both engines |

FIFO words are durations in carrier periods: mark `0x8000 | n`, space `n`,
a `0x4000 | (n >> 14)` high word first when `n > 0x3fff`, and `0xc000` to end.
To send: program the registers, pulse reset, fill the FIFO, set GO. GO clears
when the train is out.

## Confirmed on hardware

With a GC-IRL learner on jack 1:

- **OE bit 0 is jack 1**, from either engine. Jacks 2–8 are presumably bits 1–7.
- **Carrier is exact.** Periods 500 / 658 / 694 / 760 / 1000 measured 100 / 76 /
  72 / 66 / 50 kHz. The vendor's default period `0x17e` is 131 kHz, past the
  learner's range, which reports it aliased as 66 kHz with halved counts.
- **The FIFO holds a whole code.** Three back-to-back NEC frames (204 words)
  loaded before GO came out intact, and NEC `0x20DF10EF` learned back at exactly
  38000 Hz.

An earlier reading had the watermark down as a jack "select", the repeat
register as a carrier prescaler, and never wrote OE, so nothing ever radiated.

## Why this matters

It is enough to write an openHC IR driver that speaks to the real FPGA registers
and replaces the vendor `c4irout` entirely — no vendor blob, no guessing. The
[bitstream loader](/iox/io-map/) and the [own-Verilog question](/iox/fpga-replacement/)
are separate; this is purely the software interface to the IR hardware once the
FPGA is configured.
