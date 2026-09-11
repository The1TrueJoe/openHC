---
title: The FPGA start-up clock
description: Why the bitstream loads cleanly but the FPGA never finishes starting up, and the DM355 VENC clock that fixes it.
sidebar:
  order: 5
---

The bitstream loads with **zero CRC errors** — INIT_B stays high through all
169 KB — and yet DONE never releases and the FPGA register bus stays floating
(the version word reads `0x0202`, not the configured `0x0004`). Sending more
start-up clocks changes nothing. This page explains why, because the answer is
non-obvious and was provable entirely offline.

## The bitstream waits for a DCM to lock

A Spartan-3E finishes configuration by running a **start-up state machine** of
eight phases (C0–C7). Where each event happens — DONE release, global write
enable, global tristate — is baked into the bitstream's **COR** (Configuration
Options Register).

Parsing the vendor bitstream (`fpga_fw.bin`), the COR is `0x000031e5`. Decoded:

| field       | value | meaning                                  |
|-------------|-------|------------------------------------------|
| LCK_cycle   | 0     | **wait for DCM lock at phase C0**        |
| DONE_cycle  | 7     | release DONE at phase C7                 |
| GTS_cycle   | 4     | global tristate at C4                    |
| GWE_cycle   | 5     | global write enable at C5                |

The bitgen "don't wait for lock" default is `0x3fe5`; the vendor image differs
from it in exactly the LCK_cycle bits (`0x0e00`). So the state machine **parks
at C0 until the design's DCM reports LOCKED**. DONE lives at C7, which is never
reached. The CCLKs we keep sending can't help — the machine is waiting on a
*reference clock into the DCM*, which comes from the board, not from us.

## The clock is the DM355 video encoder

The IO Extender has no display, but its DM355 **video encoder (VENC)** is wired
to the FPGA purely as a clock source — the FPGA's DCM runs off the VENC digital
LCD clock (**DCLK**).

The vendor turns this on **in U-Boot**, before it loads the part, in the GPL
sources `c4fpgaldr.c` / `davincifb.c` (`enableDigitalOutput()`). Then Linux
boots with the FPGA already configured and the clock already ticking. openHC
netboots straight to `bootm` and loads the FPGA from the *kernel* instead, so
that U-Boot video init never runs and the DCM never sees a clock.

## Enabling it from the kernel

`ohc-iox-fpga` now starts the VENC DCLK at probe, with the vendor's exact
register writes:

| register        | address       | value            | purpose                     |
|-----------------|---------------|------------------|-----------------------------|
| PSC MDCTL[0,1]  | `0x01c41a00`/`…a04` | ENABLE     | power VPSS master + slave   |
| VPSS_CLKCTL     | `0x01c40044`  | `0x0a`           | route the VPSS clock        |
| VPBE_PCR        | `0x01c72784`  | `0`              | full (undivided) clock      |
| VENC_VIDCTL     | `0x01c72404`  | `0x2000` (VLCKE) | enable video clock output   |
| VENC_DCLKCTL    | `0x01c72464`  | `0x0800` (DCKEC) | enable the digital clock    |
| VENC_DCLKPTN0/0A| `0x01c72468`/`…78` | `1` / `2`   | DCLK pattern (≈ /2 square)   |
| VENC_DCLKHSA    | `0x01c7248c`  | `1`              | DCLK pattern                |
| VENC_OSDCLK0/1  | `0x01c7252c`/`…30` | `0` / `1`   | OSD clock                   |

:::note[VPSS_CLKCTL is at 0x01c40044]
Earlier bring-up notes had the VPSS clock control at `0x01c70200`; that was
wrong. The register that matters is in the **system module** at `0x01c40044`,
and the vendor writes `0x0a` to it.
:::

openHC runs no DaVinci PSC clock driver, so once the domain is enabled nothing
gates it back off — the clock free-runs, which is what the FPGA's UART logic
needs regardless. With the clock present, the DCM locks, the C0 stall clears,
the machine advances to C7, and DONE releases.
