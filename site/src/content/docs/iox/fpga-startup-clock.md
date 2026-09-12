---
title: The FPGA startup problem
description: The bitstream loads cleanly but the FPGA never finishes starting up. What's been ruled out, and why the next step is JTAG.
sidebar:
  order: 5
---

The bitstream loads with **zero CRC errors** — INIT_B stays high through all
169 KB (verified with an explicit per-byte INIT_B gate). But **DONE never
releases** and the version register floats at `0x0202` (a configured part reads
`0x0400`). The data is accepted; the **startup sequence never completes**.

This page records what has been ruled out, so the investigation isn't repeated.

## Not the video/VENC clock

An earlier theory held that the FPGA's DCM needs the DM355 video encoder (VENC)
clock, stripped along with video in openHC. **Disproven.** On the live vendor OS,
where the FPGA works, `VPSS_CLKCTL` is `0` and the VENC block reads floating and
unclocked — identical to openHC. The FPGA is configured there regardless. The
"no video → no clock" correlation was a coincidence.

## Not the driver or the load sequence

A standalone **userspace bit-bang** (mmap `/dev/mem`, replicating the vendor's
exact slave-serial sequence, bypassing the kernel driver) fails **identically**
to the in-kernel loader. So the failure is not in the driver.

The load sequence itself was disassembled from the vendor's `c4fpga.ko` and
matched byte for byte: PROG polarity (high = reset, low = release — confirmed on
hardware via INIT_B), M2/M0 high, DIN sampled on the rising CCLK edge, the GPIO
SET/CLR register offsets, and the post-data startup clocks.

## Not any SoC register

openHC and the working vendor OS were compared register-for-register and are
**identical**:

| block | result |
|-------|--------|
| system module 0x01c40000–0x70 | identical |
| PLLC1 (0x01c40900) full | identical |
| PLLC2 (0x01c40d00) full | identical |
| PSC MDSTAT[0..41] | identical (incl. VPSS master/slave) |
| PINMUX0–4 | identical |
| async EMIF CS1 (`A2CR`, FPGA bus) | identical (`0x00a00505`) |
| bitstream `fpga_fw.bin` | identical md5 |

## Not clocking regularity

Clocking the entire stream with interrupts disabled (gap-free CCLK, to rule out
a DCM losing its reference to preemption) did not help either.

## What's left: the pins

Same board, same bitstream, byte-identical register state, same sequence, clean
data — yet the vendor completes startup (DONE high) and openHC does not. Every
remotely-observable difference has been eliminated. The remaining unknowns are
only visible with instrumentation: is the FPGA's DCM reference clock actually
present, and what do DONE / INIT_B / CCLK do during startup?

**The next step is JTAG or a scope on the J18 header** — compare the vendor and
openHC at the FPGA pins during a load. Until then, serial (`ttyS1..4`) and the IR
block stay offline; relays, contacts, LEDs, `webd` and MQTT `iod` are unaffected
and working.
