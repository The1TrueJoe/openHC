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

## Two blocks, eight halfword registers each

The IR output lives in two 16-byte windows on the FPGA, `0x04000220` and
`0x04000230` (confirmed in `/proc/iomem` as `c4irout.0`). Each is eight halfword
registers. `c4irout_config` selects which block an emitter uses via a "mode":

- mode 0 → block at base + 0x20
- mode 1 → block at base + 0x30

## Register roles (from c4irout_setup / c4irout_config / c4irout_go)

Per emitter, `c4irout_config` assigns the register offsets and `c4irout_setup`
writes them from a config struct:

| Register | Written from | Meaning |
|---|---|---|
| CONTROL | read-modify-write | bit 15 = **go**, bit 14 = mode flag, bit 13 always set, bits 5–12 a select field |
| carrier | `(cfg & 0x7f) << 9` | carrier prescaler (not direct Hz) |
| count | `cfg - 1` | burst length minus one |
| data/A/B | direct | timing/data words |
| status | `+0x3e` | shared status/version |

To send: program carrier, count and data; write CONTROL with the mode/enable
bits; then `c4irout_go` sets **bit 15** of CONTROL to fire. Completion raises the
shared FPGA interrupt on GIO7 (IRQ 71).

## What still needs a live confirm

Two calibration details, settleable in minutes once openHC's own FPGA driver can
drive the block:

- the exact carrier math — `(v & 0x7f) << 9` looks like a prescaler; fire a
  known 38 kHz code on the vendor OS and read the register back to calibrate;
- which physical jack is mode 0 vs mode 1, and how eight emitters map onto two
  blocks (likely the CONTROL select field, bits 5–12).

## Why this matters

It is enough to write an openHC IR driver that speaks to the real FPGA registers
and replaces the vendor `c4irout` entirely — no vendor blob, no guessing. The
[bitstream loader](/iox/io-map/) and the [own-Verilog question](/iox/fpga-replacement/)
are separate; this is purely the software interface to the IR hardware once the
FPGA is configured.
