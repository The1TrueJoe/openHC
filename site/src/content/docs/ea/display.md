---
title: Display registers
description: The CE5300 display controller, its HDMI transmitter and the display PLL, mapped for a KMS driver.
sidebar:
  order: 10.5
---

The EA's picture today is whatever CEFDK left: 720x480p on pipe A, scanned out of
`0x7fc00000` and described to Linux as a simple framebuffer (see
[graphics](/ea/graphics/#the-720x480-ceiling-and-why)). Hotplug, EDID and real
modes need a driver that owns three blocks:
- the display controller (VDC);
- the HDMI transmitter;
- the display PLL.

This page is the map for that driver. It comes from the symbol names and
register tables in stock's `gdl_server.ko` and `pd_hdmi.ko`, which both survive
in the recovery image on every EA's p2.

**Status** marks what has been seen on hardware. Everything else is static
reading and needs a named-register check first.

:::caution[Never sweep these BARs]
A read-only sweep of the VDC BAR hung an EA-3 outright: holes in the window
are not safe to read. Read only the registers named here, one at a time, from a
kernel driver. Don't touch the sideband from userspace either: byte-wise
writes to the host bridge's `0xd0` mailbox from a shell hung an EA-1, because
every byte write can fire a half-written command.
:::

## HDMI transmitter (`01:08.2`, `8086:2e63`)

BAR 0, 64 KiB. Audio (the transmitter's own DMA, infoframes, GCP) is on the
[audio page](/ea/audio/#hdmi-out-every-ea).

| Register | Meaning | Status |
|---|---|---|
| `+0x1000` | clocks: bit 0 TMDS, 1 pixel, 2 audio, 3 the 5 V supply to the connector, 5 HDCP | read: `0x2f` under CEFDK |
| `+0x1004` | interrupt mask | read: 0 |
| `+0x1008` | status: bit 0 **HPD**, bit 1 RX sense (`hdmi_general_get_status`) | read |
| `+0x100c` | interrupt status, latched whether masked or not, write 1 to clear | W1C seen |
| `+0x3000` | bit 0 HDMI mode (clear = DVI), bit 7 unit enable | read |

Interrupt status bits, named from `pd_interrupt_handler`'s log strings:

| Bit | Event |
|---|---|
| 0 | hotplug (`DETECT`/`LOSS` from status bit 0) |
| 1 | RX sense |
| 2 | DDC transaction done |
| 3 | DDC buffer full (64 bytes) |
| 4 | DDC bus error |
| 5 | audio buffer |
| 13, 14, 19 | HDCP R0, M0, Ri ready |
| 15 | frame |

### DDC master

The SoC's four I²C buses (the pxa controllers at `01:0b.2`) don't reach the
HDMI connector: none of them answers at the EDID address. The transmitter has
a DDC master of its own (`hdmi_i2c_*`):

| Register | Meaning |
|---|---|
| `+0x1100` | control: `0x4000` resets it, `0x8760` is `hdmi_i2c_enable` |
| `+0x1104` | status: bits 2/3 busy; bit 1 seen after a NAK |
| `+0x110c` | bus monitor: `3` = SCL and SDA high, idle |
| `+0x1110` | speed: `0x1f` |
| `+0x1200`–`+0x123f` | 64-byte data buffer |
| `+0x1240` | command: length `<< 20`, offset `<< 4`, bit 3 read, bit 1 continue, bit 0 go |
| `+0x1254`, `+0x1258` | write and read slave address. Reset value `0x74`/`0x75` (HDCP). EDID is `0xa0`/`0xa1` |

A read sets the two addresses, then writes the command with go set. Each
time 64 bytes are in, the interrupt status shows **buffer full**; read the
buffer, clear the bit, and set **continue**. The last chunk ends with **done**
(or buffer full again). A missing sink is a **bus error**.

Seen on hardware:
- with no sink, a read at `0xa0` ends in a bus error;
- `ce5300-hdmi` drives this block as an i2c adapter, and on an EA-3 with no display it NAKs cleanly.

## Display controller (`01:08.0`, `8086:2e61`)

BAR 0, 1 MiB at `0xdfb00000`.

### Pipes

Pipe A is the HDMI pipe, in the i9xx layout. Each word is
`(end - 1) << 16 | (start - 1)` (`vdc_pipe_timings_set`):

| Register | Meaning | Status |
|---|---|---|
| `0x60000` | HTOTAL (active, total) | read: `0x035902cf` = 720 of 858 |
| `0x60004` | HBLANK | read |
| `0x60008` | HSYNC | read |
| `0x6000c` | VTOTAL | read: `0x020c01df` = 480 of 525 |
| `0x60010` | VBLANK | read |
| `0x60014` | VSYNC | read |
| `0x6001c` | PIPESRC, `(hactive - 1) << 16 \| (vactive - 1)` | |
| `0x60028` | vsync shift (interlaced modes) | |
| `0x70008` | PIPEACONF: bit 31 enable, 19 planes on, 20 component, 0 dither | read: `0x80000000` |
| `0x70024` | PIPEASTAT | read, changing |

Pipe B, the TV pipe, has a reduced set:
- `0x68500` hactive, `0x68504` hblank, `0x6850c` vactive, `0x68510` vblank;
- PIPEBCONF is `0x68550` (bits 31 and 19), and pipe B's dither is `0x71008`.

The blender (`0x50000` pipe A, `0x50004` pipe B) and the canvas color
(`0x50008`, `0x5000c`) sit in front of the pipes.

### Planes

The planes aren't the i9xx DSP registers. GDL ids `0x101`–`0x109` map onto
three plane types, each at a block base (`vdc_get_plane`):

| Type | GDL ids | Registers | Formats |
|---|---|---|---|
| UPP, universal pixel plane | `0x101`–`0x105` | `0x30000` + `0xf000`, `0`, `0x1000`, `0x2000`, `0x3000` | RGB and YUV |
| IAP, indexed/alpha plane | `0x106`–`0x108` | `0x10000` + `0xf000`, `0`, `0x1000` | 8-bit indexed, A1/A4/A8 only |
| cursor | `0x109` | `0x40000` | |

CEFDK's `a8r8g8b8` console is on the UPP at `0x31000`: `ce5300-drm` read its
control as `0x08ff4c01` (enabled, pipe A, ARGB 32), its surface as
`0x7fc00000` and its pitch as 2944 on an EA-3. Per UPP, from
`__init_upp_reg_table` and `vdc_plane_buffers_set`:

| Offset | Meaning |
|---|---|
| `+0x168` | control: bit 0 enable, bit 3 pipe B, bits 14:10 pixel format |
| `+0x100`, `+0x104`, `+0x108`, `+0x10c` | surface addresses (Y, then chroma). `+0x100` read `0x7fc00000` |
| `+0x118` | pitch in bytes (read 2944) |
| `+0x064` | read 0 on the console plane (YUV pitches?) |
| `+0x164`, `+0x16c`, `+0x178`, `+0x00c` | further control and attribute bits |
| `+0x12c`–`+0x150` | keys and alpha |
| `+0x200`–`+0x23c` | the scaler |
| `+0x900`, `+0x300` | gamma and LUT |

UPP pixel-format codes (the field in `+0x168`):

| Format | Code |
|---|---|
| ARGB 32 | `0x4c00` |
| RGB 32 | `0x4800` |
| RGB 24 | `0x4000` |
| RGB 30 | `0x4400` |
| RGB 36 | `0x7000` |
| ARGB 1555 | `0x5800` |
| ARGB 4444 | `0x5c00` |
| RGB 16 | `0x5400` |
| RGB 15 | `0x5000` |

An IAP's control word is at `0x10168` + base (enable bit 0, pipe B bit 3). Its
surface is at `+0x100`, its pitch at `+0x064` and its size at `+0x118`.

## Display PLL (IOSF sideband port `0x8a`)

The dot clock is the display PLL on the sideband, next to the audio PLL on
port `0x8b`. The sideband is reached through the host bridge's `0xd0`/`0xd4`/
`0xd8` mailbox, the same path the HDMI audio driver uses to check the audio
PLL. `__config_ce5300` sets a pixel clock like this:

1. Register 1 `|= 0x06000000`, then latch (write 1 to register 0).
2. Write registers 0–7 from the table, with bit 26 of register 1 held set; latch.
3. Wait 1 ms.
4. Write registers 0–7 again with bit 26 clear.
5. For entries whose flag matches the board, modify register `0x11` (mask `0x1c`, value `0x80`).
6. Latch.

Register 0's bits 23–24 are always masked off. The 1.001 clocks are looked up
by their rounded value (74176 → 74175, 148352 → 148350).

| kHz | flag | registers 0–7 |
|---|---|---|
| 27000 | 0 | `01802116 0005002c 045811a0 4045800c 04045800 00404100 02040440 01424001` |
| 27027 | 0 | `01802116 0005002c 045811a0 4045800c 04045800 00404100 2e040440 01424001` |
| 54000 | 0 | `01802116 0005002c 045811a0 40458008 04045800 00404100 02040440 01424001` |
| 74175 | 0 | `01903916 00040b19 046831a0 4044002c 04045800 000041c0 22280468 01424039` |
| 74250 | 0 | `01802116 0005002c 044011a0 80440008 04045800 00404100 02040440 01424001` |
| 148350 | 0 | `01903916 00040b19 04683da4 40440028 04045800 000041c0 22280468 01424039` |
| 148500 | 1 | `00002116 00000e0b 043811a0 40440024 04045800 00804140 12040400 01424001` |
| 148500 | 0 | `01802116 0005002c 04401da4 40440004 04045800 00404100 02040440 01424001` |
| 297000 | 0 | `01802116 0005002c 04041da4 4044000c 04045800 00404100 02040440 01424001` |

The table also has 25175, 25200, 40000, 59340, 59400, 65000, 79700, 85000,
108000, 118681, 118800 and 296700 kHz. None of it has been read back on
hardware yet.

## HDMI TX PLL and PHY (`pd_hdmi.ko` `__program_clocks_ce5300`)

The transmitter's PHY is on IOSF sideband port `0x82`. Its resets are in the
clock-control unit, BAR 0 of `00:00.2` (`8086:2e52`, `0xcc` bytes), which
`clock_control.ko` maps:

| Clock-control register | Bit | Name |
|---|---|---|
| `+0x04` | 9 | `CLOCK_HDMI_PLL_DIV_RESET` |
| `+0x04` | 7 | `CLOCK_HDMI_TX_PLL_RESET` |
| `+0x04` | 1 | `CLOCK_HDMI_TX_RST` |
| `+0x04` | 0 | `CLOCK_HDMI_I2C_RESET` |
| `+0x14` | 9, 8, 7, 6, 5 | `CLOCK_VDC_DB/DA/MDVO/XSI_CLK_EN`, `CLOCK_VDC_CLK_EN` |
| `+0x14` | 0 | `CLOCK_VDC_RST` |
| `+0x88` | 3, 2 | `CLOCK_DPLL_DIV_RST`, `CLOCK_DPLL_VCO_RST` |

A pixel clock is set like this (`_clock_control_write(id, value)`; 0 asserts a
reset):
1. PLL divider reset 0, TX PLL reset 0.
2. Write the 23 PHY words for the clock (`hdmi_phy_cfg_raw`). Word 2 is
   written with bit 27 set; after word 22, word 12 (`+0x30`) is masked with
   `0x1ffefffb | 0xe0000004`, and word 2 is written again with bit 27 clear.
3. TX PLL reset 1, wait 300 µs; PLL divider reset 1, wait 50 µs.
4. TX reset 0, wait 1 ms; TX reset 1, wait 1 ms.

The PHY words are chosen by color depth (8/10/12 bpc tables at `.rodata`
`0x1c0`/`0x260`/`0x300`) and then by pixel clock. At 8 bpc, only the first
three words (PLL dividers) and words 7–11 (drive) differ between clocks:

| kHz | words 0–2 | words 7–11 |
|---|---|---|
| 27000 | `00105800 00000a14 00050005` | `02a05400 e8055030 ×4` |
| 74250 | `00002000 00050028 00090080` | `02a05400 e8055030 ×4` |
| 148500 | `00002000 00050028 00050080` | `02a05400 e8055030 ×4` |
| 297000 | `00101800 0005000f 00010080` | `02004000 c80770e7 ×4` |

The other words, all clocks:
- words 3–6: `00100000 40214000 80000001 0007f03f`;
- words 12–22: `ff001ffe ffffffff bc0000ff 36d8fff7 00100000 ffd00000 00100000 1c010000 0 0 00010101`.

A full modeset for the M4 driver is therefore:
- the display PLL (port `0x8a`) for the dot clock;
- the TX PLL and PHY (port `0x82`, plus the resets above) for the TMDS clock;
- pipe A's timing and PIPESRC;
- the HDMI line timing and GCP position (`+0x3100`/`+0x3104`, see audio);
- the AVI infoframe.

None of the sideband writes have been tried on hardware.

