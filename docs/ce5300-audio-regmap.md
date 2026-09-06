# CE5300 audio register map — recovered from `ismdaudio.ko`

Everything here is extracted from the stock module's **DWARF debug info** and
Ghidra decompilation. Names in `CAPS` / `camelCase` are Intel's own, from the
retained debug symbols (source files `audio_ce53xx_hal.c/.h`, `audio_hal_render.c`,
`audio_hal_capture.c`, `audio_hal_defs.h`, `audio_hal_defs_pvt.h`).

**Why this matters:** the SoC audio DSPs run signed firmware we cannot replace,
but the **I²S render path does not use them** — `audio_hal_render_start` contains
zero DSP/firmware/auth references. It is a conventional linked-list DMA + I²S
engine, which a normal ASoC platform driver can drive directly.

## PCI

| device | role |
|---|---|
| `8086:2e5f` | audio (claimed by `ismdaudio`, id table at `.data+0x140`) |
| `8086:2e60` | audio (second function) |

`ismdaudio`'s `pci_probe` is a **3-byte stub** — it does not map registers there.
The MMIO base comes from `os_map_io_to_mem_nocache` with a **0x300000 (3 MB)**
window; the decompile calls the base `aud_io_phys_base_addr`, device name
`"AUD_IO"` (SVEN module `SVEN_module_GEN3_AUD_IO`).

## Register blocks (offsets from `aud_io_phys_base_addr`)

Three indexed blocks, each with a clean stride:

```
DMA block   base 0x1000   stride 0x40    index = dma_context  (0..4)
TX block    base 0x2000   stride 0x100   index = tx_context   (0..2)
RX block    base 0x3000   stride 0x100   index = rx_context
```

Confirmed offsets:

| expression | meaning |
|---|---|
| `0x1010 + dma*0x40` | `srcdma_start` — DMA source start |
| `0x1038 + dma*0x40` | `srcdma_stop` — DMA source stop |
| `0x101c + dma*0x40` | primary DMA control/status (by far the most-written reg: 55 sites) |
| `0x2004 + tx*0x100` | TX control (12 sites) |
| `0x2018 + tx*0x100` | `txSATRAddr` — TX start-address register |
| `0x3018 + rx*0x100` | `rxSARRAddr` — RX start-address register |

Other fixed offsets seen in register calls: `0x2800`, `0x3000`, `0xc000`, `0xf000`.

Accessors are `devh_WriteReg32 / ReadReg32 / OrBitsReg32 / AndBitsReg32 /
SetMaskedReg32` (imported from OSAL; they take the `os_devhandle_t` plus a
byte offset — note they are `__regparm3`, so Ghidra frequently drops the
argument list and the offsets must be read from the surrounding expressions).

## Per-stream context map

`audio_hal_render_init_context(…, hw_dev_id, …)`:

| `hw_dev_id` | tx_context | dma_context | `interrupt_mask` |
|---|---|---|---|
| `0x3d1` | TX0 | DMA0 | `0x08` |
| `0x3d2` | TX1 | DMA1 | `0x20` |
| `0x3d3` | TX2 | DMA2 | `0x80` |

A further id `0x3d5` exists (separate branch). `render_context->dma_context << 0x11`
is written via `SetMaskedReg32` — i.e. the DMA context selector sits at bit 17 of
the TX register.

## Structures (verbatim from DWARF)

```c
/* audio_hal_defs_pvt.h — the scatter-gather descriptor */
struct AUDIO_HAL_DMA_DESCRIPTOR {      /* 20 bytes */
    int32_t NEXT_DESC;                 /* +0x00  linked list */
    int32_t SRCDMA_SIZE;               /* +0x04 */
    int32_t SRCDMA_START;              /* +0x08 */
    int32_t DSTDMA_START;              /* +0x0c */
    int32_t FLAGS_MODE;                /* +0x10 */
};

/* audio_hal_defs.h — the ring, note the register addresses are per-stream */
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

/* audio_ce53xx_hal.h */
struct audio_pll_configuration { uint32_t apll_cr[8]; };
struct audio_hal_adac_coff_t   { uint32_t adac_coff_reg_value[8]; };
```

## Enums (verbatim from DWARF)

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

## What a driver has to do (from `audio_hal_render_start`)

1. If `buffer_mode == LINKED_LIST`: three `WriteReg32` (descriptor base / size / mode).
2. Then `OrBitsReg32` → `ReadReg32` → `WriteReg32` on the TX block: enable, read
   the interrupt-mask register (`imrx_val`), write back with this stream's
   `interrupt_mask` bit set.

Stop/reset mirror this. `audio_hal_poll_dma_complete` shows the completion path
can be polled rather than interrupt-driven, which is a simpler first target.

## The MMIO base — RESOLVED (read off a live EA3)

`aud_io_phys_base_addr` is `devh->devh_regs_phys_addr` in the OSAL handle
(`os_devhandle`, +0x14; `devh_regs_ptr` +0x10 is the virtual mapping,
`devh_regs_size` +0x18). That is just the device's PCI BAR0. Measured on the
live EA3:

| PCI | id | BAR0 | size |
|---|---|---|---|
| `01:06.0` | `8086:2e5f` | `0xdfd00000` | 512 KB |
| `01:06.1` | `8086:2e5f` | `0xdfd80000` | 512 KB |
| `01:06.2` | `8086:2e60` | `0xdfa80000` | 64 KB |

The map is consistent with this: the DMA (`0x1000`), TX (`0x2000`), RX (`0x3000`)
blocks and the fixed `0xf000` all sit inside a 512 KB window. So a mainline
driver can simply `pcim_iomap(pdev, 0, 0)` — no OSAL needed.

## Bit fields — DECODED

`audio_hal_write_vsymbol(devh, offset, bitpos, width, mask, value, offset, …)`
takes 5-tuples terminated by zeros. **Every mask in every call satisfies
`mask == ((1 << width) - 1) << bitpos`** — nine fields in one call, six in
another, six in a third, with no exceptions. That self-consistency is what makes
the decode trustworthy rather than a guess.

### DMA control — `0x101c + dma*0x40`

From `audio_pvt_hal_dma_set_circ_buf` (circular-buffer setup):

| bits | width | value set | note |
|---|---|---|---|
| `[30]` | 1 | 1 | enable |
| `[29]` | 1 | 1 | enable |
| `[7]` | 1 | 1 | |
| `[6:5]` | 2 | 1 | |
| `[4]` | 1 | 0 | |
| `[2:1]` | 2 | 3 | |
| `[11:8]` | 4 | 0 | burst/size field (see below) |
| `[15:12]` | 4 | 0 | |
| `[19:16]` | 4 | 0 | |

From `audio_hal_set_dma_flags` (the start/arm sequence) the same register is
written with `[11:8] = 6`, `[30] = 1`, `[29] = 1`, `[6:5] = 1`, `[2:1] = 3`,
`[7] = 1`. So `[11:8]` is the field that differs between "configured" (0) and
"running" (6) — the prime candidate for the DMA burst/transfer size that
`audio_hal_render_set_dma_burst_size` sets.

### TX control — `0x2004 + tx*0x100`

From `audio_pvt_hal_render_linked_list_dma_setup`, which passes an array `pin[]`
and a `dsm`:

| bits | width | source | meaning |
|---|---|---|---|
| `[15:14]` | 2 | `pin[2]` | I²S pin 2 enable |
| `[13:12]` | 2 | `pin[1]` | I²S pin 1 enable |
| `[11:10]` | 2 | `pin[0]` | I²S pin 0 enable |
| `[9:8]` | 2 | `dsm` | data storage mode |
| `[6:5]` | 2 | `pin[3]` | I²S pin 3 enable |
| `[0]` | 1 | 0 | run/enable |

The 2-bit pin fields take `AUDIO_HAL_I2S_PIN_ENABLE` (ENABLE=0, LEFT=1, RIGHT=2,
DISABLE=3) — four pins, matching `audio_hal_i2s_pin_en_t[4]` in the DWARF. `dsm`
takes `AUDIO_HAL_DATA_STORAGE_MODE` (7_1=0, STEREO=1, LEFT=2, RIGHT=3). So for
plain stereo out: `dsm = 1`, `pin[0] = ENABLE`, the rest `DISABLE`.

Also from the DWARF: `render_context->dma_context << 0x11` is written with
`SetMaskedReg32`, i.e. the DMA-context selector is at bit 17 of the TX register.

## Still to determine
* Clocking: `audio_pvt_ce53xx_hal_set_pll_configuration_mode` + `apll_cr[8]`
  drive the audio PLL; the ADAU wants a matching MCLK (`SYSCLK_44_1K` /
  `SYSCLK_48K` in the published `ninjago-smd-dsp.c`).

## The other half is already GPL

The codec, SigmaDSP core, DAI and machine driver are published in
`kernel/c4_kernel_patches/0012-ninjago-alsa-smd-fpga-codec.patch` (33 files,
zero `ismd_` references) — including `ea3-dsp-params.h`, 10,546 lines of the
EA3's DSP parameter map. Only the PCM/DMA platform component is missing, and
this map is what is needed to write it.
