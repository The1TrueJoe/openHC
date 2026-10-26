---
title: CE5300 audio register map
description: The I2S and DMA register map recovered from ismdaudio.ko's DWARF debug info.
sidebar:
  order: 9
---

Everything here is extracted from the stock module's **DWARF debug info** and
Ghidra decompilation. Names in `CAPS` and `camelCase` are Intel's own, from the
retained debug symbols, source files `audio_ce53xx_hal.c/.h`,
`audio_hal_render.c`, `audio_hal_capture.c`, `audio_hal_defs.h`.

**Why this matters:** the SoC audio DSPs run signed firmware we can't replace,
but the **I²S render path doesn't use them** — `audio_hal_render_start` contains
zero DSP, firmware or auth references. It's a conventional linked-list DMA plus
I²S engine, which a normal ASoC platform driver can drive directly.

## PCI

| Device | Role |
|---|---|
| `8086:2e5f` | audio (claimed by `ismdaudio`) |
| `8086:2e60` | audio (second function) |

`ismdaudio`'s `pci_probe` is a **3-byte stub**, it doesn't map registers there.
The MMIO base comes from an OSAL mapping call with a **3 MB** window; the
decompile calls the base `aud_io_phys_base_addr`, device name `"AUD_IO"`.

### The MMIO base — resolved off a live EA3

`aud_io_phys_base_addr` is `devh->devh_regs_phys_addr` in the OSAL handle. That
is just the device's PCI BAR0:

| PCI | ID | BAR0 | Size |
|---|---|---|---|
| `01:06.0` | `8086:2e5f` | `0xdfd00000` | 512 KB |
| `01:06.1` | `8086:2e5f` | `0xdfd80000` | 512 KB |
| `01:06.2` | `8086:2e60` | `0xdfa80000` | 64 KB |

The map is consistent with this, the DMA (`0x1000`), TX (`0x2000`) and RX
(`0x3000`) blocks and the fixed `0xf000` all sit inside a 512 KB window. So a
mainline driver can simply `pcim_iomap(pdev, 0, 0)`; no OSAL needed.

## Register blocks

Three indexed blocks, each with a clean stride:

```
DMA block   base 0x1000   stride 0x40    index = dma_context  (0..4)
TX block    base 0x2000   stride 0x100   index = tx_context   (0..2)
RX block    base 0x3000   stride 0x100   index = rx_context
```

Confirmed offsets:

| Expression | Meaning |
|---|---|
| `0x1010 + dma*0x40` | `srcdma_start` — DMA source start |
| `0x1038 + dma*0x40` | `srcdma_stop` — DMA source stop |
| `0x101c + dma*0x40` | primary DMA control/status (55 write sites — by far the most-written) |
| `0x2004 + tx*0x100` | TX control (12 sites) |
| `0x2018 + tx*0x100` | `txSATRAddr` — TX start-address register |
| `0x3018 + rx*0x100` | `rxSARRAddr` — RX start-address register |

Other fixed offsets seen: `0x2800`, `0x3000`, `0xc000`, `0xf000`.

Accessors are `devh_WriteReg32` / `ReadReg32` / `OrBitsReg32` / `AndBitsReg32` /
`SetMaskedReg32`. Note they are `__regparm3`, so Ghidra frequently drops the
argument list and the offsets must be read from the surrounding expressions.

## Per-stream context map

| `hw_dev_id` | tx_context | dma_context | `interrupt_mask` |
|---|---|---|---|
| `0x3d1` | TX0 | DMA0 | `0x08` |
| `0x3d2` | TX1 | DMA1 | `0x20` |
| `0x3d3` | TX2 | DMA2 | `0x80` |

A further id `0x3d5` exists on a separate branch.
`render_context->dma_context << 0x11` is written via `SetMaskedReg32`, the DMA
context selector sits at **bit 17** of the TX register.

## Structures, verbatim from DWARF

```c
/* the scatter-gather descriptor */
struct AUDIO_HAL_DMA_DESCRIPTOR {      /* 20 bytes */
    int32_t NEXT_DESC;                 /* +0x00  linked list */
    int32_t SRCDMA_SIZE;               /* +0x04 */
    int32_t SRCDMA_START;              /* +0x08 */
    int32_t DSTDMA_START;              /* +0x0c */
    int32_t FLAGS_MODE;                /* +0x10 */
};

/* the ring — note the register addresses are per-stream */
struct audio_render_circbuf_t {        /* 40 bytes */
    uint      base;                    /* +0x00 */
    uint      size;                    /* +0x04 */
    uint32_t  read_reg_addr;           /* +0x08 */
    uint32_t  write_reg_addr;          /* +0x0c */
    uint32_t  saved_read_ptr_val;      /* +0x10 */
    uint32_t  saved_write_ptr_val;     /* +0x14 */
    uint32_t *virt_write_ptr;          /* +0x18 */
    uint32_t *virt_read_ptr;           /* +0x1c */
    uint8_t  *virt_buf_ptr;            /* +0x20 */
    uint      phys_end_addr;           /* +0x24 */
};
```

## Enums, verbatim from DWARF

```c
AUDIO_HAL_OUTPUT_SEL   : I2S1=0, I2S0=1, NOT_USED=2, SPDIF=3
AUDIO_HAL_INPUT_SEL    : I2S1=0, I2S0=1, SPDIF=2
AUDIO_HAL_I2S_WS_SELECT: FALLING=0, RISING=1
AUDIO_HAL_I2S_PIN_ENABLE: ENABLE=0, LEFT=1, RIGHT=2, DISABLE=3
AUDIO_HAL_I2S_INPUT_TYPE: MONO_OR_STEREO=0, RESERVED=1, 5_1=2, 7_1=3
AUDIO_HAL_ALT_MODE     : I2S=0, MSB_JUSTIFIED=1
AUDIO_HAL_DATA_STORAGE_MODE: 7_1=0, STEREO=1, LEFT=2, RIGHT=3
AUDIO_TX_CONTEXT       : TX0=0, TX1=1, TX2=2, COUNT=3
AUDIO_DMA_CONTEXT      : 0..4, COUNT=5
AUDIO_RENDER_BUFFER_MODE: LINKED_LIST=0
AUDIO_IO_STATE         : RESET=0, RELEASE=1
AUDIO_HAL_PLL_MODE     : MODE1=0, MODE4=1, MODE7=2, MODE12=3, MODE23=4, MODE15=5, MODE16=6
```

## Bit fields — decoded

The vendor writes fields through a helper taking 5-tuples terminated by zeros.
**Every mask in every call satisfies `mask == ((1 << width) - 1) << bitpos`** —
nine fields in one call, six in another, six in a third, with no exceptions. That
self-consistency is what makes the decode trustworthy rather than a guess.

### DMA control — `0x101c + dma*0x40`

From the circular-buffer setup path:

| Bits | Width | Value set | Note |
|---|---|---|---|
| `[30]` | 1 | 1 | enable |
| `[29]` | 1 | 1 | enable |
| `[7]` | 1 | 1 | |
| `[6:5]` | 2 | 1 | source address mode |
| `[4]` | 1 | 0 | |
| `[2:1]` | 2 | 3 | destination address mode |
| `[11:8]` | 4 | 0 | burst/size field |
| `[15:12]` | 4 | 0 | |
| `[19:16]` | 4 | 0 | |

From the start/arm sequence the same register is written with `[11:8] = 6` and
everything else the same. So **`[11:8]` is the field that differs between
"configured" (0) and "running" (6)** — the prime candidate for the DMA burst size.

### TX control — `0x2004 + tx*0x100`

| Bits | Width | Source | Meaning |
|---|---|---|---|
| `[15:14]` | 2 | `pin[2]` | I²S pin 2 enable |
| `[13:12]` | 2 | `pin[1]` | I²S pin 1 enable |
| `[11:10]` | 2 | `pin[0]` | I²S pin 0 enable |
| `[9:8]` | 2 | `dsm` | data storage mode |
| `[6:5]` | 2 | `pin[3]` | I²S pin 3 enable |
| `[0]` | 1 | 0 | run/enable |

The 2-bit pin fields take `AUDIO_HAL_I2S_PIN_ENABLE` and `dsm` takes
`AUDIO_HAL_DATA_STORAGE_MODE`. **For plain stereo out: `dsm = 1`, `pin[0] =
ENABLE`, the rest `DISABLE`.**

## Measured on a stock unit while it plays

Read off a running stock EA3 (2026-10-26) through `/dev/mem`, with stock's own
`ismdaudio` driving the hardware and `aplay` playing silence at 48 kHz/16-bit
into card 0's `analog0`. Stock keeps this pipeline running even when nothing
plays, so the block is clocked and reads are safe there; on openHC it is not
(see the warning below). Read-only throughout.

**The base is right.** Stock maps `AUD_IO` as **64 KB at 0xdfa80000** — the
`8086:2e60` function's BAR0 (01:06.2), the same base `ce5300-i2s` maps. The two
`8086:2e5f` functions get only 8 KB each; they are the audio DSP cores, not the
I²S path.

Static values while playing:

| Register | Value | Reading |
|---|---|---|
| `+0x0000` | `0x00000007` | global — reads `0x1` under openHC; bits 1–2 are the first suspects for the sub-block enables |
| `+0x000c` | `0x00000008` | global — exactly DMA0's per-stream `interrupt_mask` bit, the likely **interrupt mask register** (`imrx`) |
| `+0x002c` | `0x00000180` | global |
| `+0x0064` | `0x00000001` | global |
| `+0x2000` TX0 | `0x00000021` | bit 0 set while running — the likely **stream enable** |
| `+0x2004` TX0 ctrl | `0x0000f1e0` | pin0 ENABLE, pins 1–3 DISABLE, dsm STEREO — as decoded — but **bit 0 is 0 while playing**, so it is not the run bit |
| `+0x2008` | `0x00000005` | |
| `+0x2010` | `0x00000100` | |
| `+0x2014` | `0x00000008` | per-context config (TX1/TX2 hold `6` at `+0x14`), not the mask |
| `+0x201c` | `0x0000003f` | |
| `+0x2020` | moving, `0x42..0x7f` | FIFO level |
| `+0x2104/2204` | `0x000e0000` | TX1/TX2, idle |
| `+0x3004` RX0 | `0x001e0000` | |
| `+0x1014` DMA0 | `0xdfa82018` | destination = `AUD_IO + 0x2018`, the TX0 FIFO port (`txSATRAddr`) |
| `+0x101c` DMA0 ctrl | `0xc0006696` | the descriptor's mode word with bit 31 (active) set |

Moving while it plays — **the DMA position**:

| Register | Reading |
|---|---|
| `+0x1008` / `+0x100c` | descriptor addresses, stepping in 32-byte units |
| `+0x1010` (= `+0x1020`) | **current source address** inside the buffer — what `.pointer` needs |
| `+0x1018` | bytes left in the current descriptor |

The descriptors themselves, read from RAM (32 bytes each, a ring):

```
+0x00 next descriptor       +0x10 mode   0x40006696
+0x04 source size  0x00000f00  +0x14 per-buffer word (varies)
+0x08 source buffer         +0x18 size again 0x00000f00
+0x0c destination 0xdfa82018 +0x1c this descriptor's own address
```

So a period here is 3840 bytes (960 stereo 16-bit frames, 20 ms at 48 kHz).

**Clock and reset.** `01:0c.1` (`8086:2e6f`, `ismdclock`, 0xdffe1000) is
timestamp counters, not gating. The SoC's Clock and Reset Unit, `00:00.2`
(`8086:2e52`), has a 256-byte register block at **0xbfffef00** that stock's
OSAL maps; its values on a playing stock unit are the reference for finding the
bits that ungate audio — read the same block on openHC and compare:

```
+00 000003fc  +04 00020283  +08 000001e3  +0c 0000f843  +10 00000017  +14 0000e7e3
+18 000000f1  +1c 0fe1030f  +20 00000023  +24 00000003  +28 00000007  +2c 00000041
+30 000001ff  +34 00000001  +38 00000099  +3c 0000003f  +40 000000e3  +44 00000003
+48 00000063  +4c 00000022  +50 00000003  +58 00000003  +5c 0000000f  +60 8000a222
+64 28f42180  +68 00018269  +80 0929e3ff  +84 009f12a7  +8c 00000047  +90 00000047
+94 000007fc  +98 000007fe   (all others 0)
```

## What a driver has to do

From `audio_hal_render_start`:

1. If `buffer_mode == LINKED_LIST`: three writes — descriptor base, size, mode.
2. Then an or-bits, a read and a write on the TX block: enable, read the
   interrupt-mask register, write it back with this stream's `interrupt_mask` bit
   set.

Stop and reset mirror this. `audio_hal_poll_dma_complete` shows the completion
path can be **polled** rather than interrupt-driven, which is a simpler first
target.

## Still to determine

**Clocking.** `audio_pvt_ce53xx_hal_set_pll_configuration_mode` and `apll_cr[8]`
drive the audio PLL, and the ADAU wants a matching MCLK.

## The other half is already GPL

The codec, SigmaDSP core, DAI and machine driver are published in Control4's own
kernel patch series — 33 files with zero `ismd_` references. **Only the PCM/DMA
platform component is missing**, and this map is what is needed to write it.
