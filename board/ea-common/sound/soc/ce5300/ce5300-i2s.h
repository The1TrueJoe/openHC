/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Register map for the Intel CE5300 audio I2S/DMA block (PCI 8086:2e60).
 *
 * Recovered from the DWARF debug info in Control4's ismdaudio.ko, which is a
 * conventional I2S + scatter-gather DMA engine. The signed SoC DSP firmware
 * drives DECODE/MIX only; the render (playback) path needs none of it, which is
 * what makes a mainline driver possible at all.
 *
 * Confidence is recorded per field below, because it is not uniform: most of it
 * is provable, a few things are inference. Anything marked INFERRED must be
 * verified against hardware before it is trusted -- a wrong bit here is silence
 * or a wedged DMA, not a compile error.
 *
 * The vendor HAL writes registers through a helper taking repeated 5-tuples of
 * (reg_offset, bit_pos, bit_width, bit_mask, value). Every observed tuple obeys
 * bit_mask == ((1 << bit_width) - 1) << bit_pos, and that redundancy is what
 * makes the field decode trustworthy rather than a guess.
 */
#ifndef _CE5300_I2S_H
#define _CE5300_I2S_H

#include <linux/bits.h>		/* BIT() */
#include <linux/types.h>	/* __le32 */

/* ---- DMA block: base 0x1000, 5 contexts, stride 0x40 ------------------- */
#define CE5300_DMA_BASE			0x1000
#define CE5300_DMA_STRIDE		0x40
#define CE5300_DMA_CONTEXTS		5
#define CE5300_DMA_REG(dma, off)	(CE5300_DMA_BASE + (dma) * CE5300_DMA_STRIDE + (off))

#define CE5300_DMA_SRCDMA_START		0x10	/* head descriptor (INFERRED, see probe) */
#define CE5300_DMA_CTRL			0x1c	/* the flags/mode word; also FLAGS_MODE in a descriptor */
#define CE5300_DMA_SRCDMA_STOP		0x38	/* tail descriptor (INFERRED) */

/*
 * DMA control word. The FLAGS_MODE field of a descriptor is a shadow copy of
 * this same layout -- the vendor builds both with the identical tuple list.
 */
#define CE5300_DMA_DST_MODE_SHIFT	1	/* [2:1] address mode, dest   */
#define CE5300_DMA_SRC_MODE_SHIFT	5	/* [6:5] address mode, source */
#define CE5300_DMA_ADDR_MODE_MASK	0x3
#define   CE5300_ADDR_LINEAR		0
#define   CE5300_ADDR_CIRCULAR		1
#define   CE5300_ADDR_FIXED		2
#define   CE5300_ADDR_FIXED_CONT	3
#define CE5300_DMA_BURST_SHIFT		8	/* [11:8]  AUDIO_HAL_DMA_BURST_SIZE  */
#define CE5300_DMA_XBURST_SHIFT		12	/* [15:12] AUDIO_HAL_DMA_XBURST_SIZE */
#define CE5300_DMA_GAP_SHIFT		16	/* [19:16] gap size; always 0 in every observed site */
#define   CE5300_BURST_256		6	/* what the vendor render path uses */
#define CE5300_DMA_FLAG_SRC_A		BIT(4)
#define CE5300_DMA_FLAG_SRC_B		BIT(7)
#define CE5300_DMA_FLAG_DST_B		BIT(3)
#define CE5300_DMA_FLAG_DST_A		BIT(0)

/*
 * Bit 28 is the whole flow-control mechanism: set means "halt after this
 * descriptor". The ring is chained ONCE and never re-linked; queueing a buffer
 * marks the new tail with STOP and clears STOP on the descriptor before it, so
 * the engine can never run into a stale descriptor. This is proven by the
 * render and capture paths flipping only this bit between their two otherwise
 * identical flag words.
 */
#define CE5300_DMA_STOP_AFTER		BIT(28)
/* Bits 29/30 are written by the vendor but their meaning is UNRESOLVED. Bit 30
 * is set and bit 29 cleared on the render path; we copy that verbatim rather
 * than reason about it. */
#define CE5300_DMA_BIT30		BIT(30)

/* Playback flag word, minus the STOP bit. Matches the vendor render path. */
#define CE5300_DMA_FLAGS_PLAYBACK					\
	(CE5300_DMA_BIT30 |						\
	 (CE5300_BURST_256 << CE5300_DMA_BURST_SHIFT) |			\
	 (CE5300_BURST_256 << CE5300_DMA_XBURST_SHIFT) |		\
	 (CE5300_ADDR_FIXED_CONT << CE5300_DMA_DST_MODE_SHIFT) |	\
	 (CE5300_ADDR_LINEAR << CE5300_DMA_SRC_MODE_SHIFT) |		\
	 CE5300_DMA_FLAG_SRC_A | CE5300_DMA_FLAG_SRC_B)

/* ---- TX block: base 0x2000, 3 contexts, stride 0x100 ------------------- */
#define CE5300_TX_BASE			0x2000
#define CE5300_TX_STRIDE		0x100
#define CE5300_TX_CONTEXTS		3
#define CE5300_TX_REG(tx, off)		(CE5300_TX_BASE + (tx) * CE5300_TX_STRIDE + (off))

#define CE5300_TX_CTRL			0x04
#define CE5300_TX_SATR			0x18	/* FIFO write port; the DMA destination */

/*
 * TX control. The I2S pin-enable fields are deliberately NOT in bit order --
 * pin3 sits low at [6:5] while pins 0..2 run upward from bit 10. Do not "tidy"
 * this into an array; the hardware really is laid out that way.
 */
#define CE5300_TX_PIN0_SHIFT		10	/* [11:10] */
#define CE5300_TX_PIN1_SHIFT		12	/* [13:12] */
#define CE5300_TX_PIN2_SHIFT		14	/* [15:14] */
#define CE5300_TX_PIN3_SHIFT		5	/* [6:5]   */
#define CE5300_TX_PIN_MASK		0x3
#define   CE5300_PIN_ENABLE		0
#define   CE5300_PIN_DISABLE		3
#define CE5300_TX_STORAGE_SHIFT		8	/* [9:8] */
#define   CE5300_STORAGE_7_1		0
#define   CE5300_STORAGE_STEREO		1
#define CE5300_TX_SAMPLE_24BIT		BIT(7)	/* 0 = 16-bit. Position good, association INFERRED */
#define CE5300_TX_DMA_CTX_SHIFT		17	/* [19:17] binds this TX stream to a DMA context */
#define CE5300_TX_DMA_CTX_MASK		0x7

/*
 * TX enable. INFERRED: render_start does a single OrBitsReg32 on this block and
 * render_stop the matching AndBitsReg32, but __regparm3 stripped which bit.
 * Bit 0 is the only 1-bit field ever written here (channel-config clears it),
 * so "cleared to reconfigure, set to run" is the natural reading. VERIFY.
 */
#define CE5300_TX_ENABLE		BIT(0)

/* ---- RX block (capture; not used by this driver yet) ------------------- */
#define CE5300_RX_BASE			0x3000
#define CE5300_RX_STRIDE		0x100
#define CE5300_RX_SARR			0x18

/* ---- Interrupts -------------------------------------------------------- */
/*
 * render_start ends with a read-modify-write of an interrupt mask register
 * (the two locals survived in DWARF as imrx_val / new_imrx_val), OR-ing in a
 * per-stream mask chosen from hw_dev_id: 0x3d1 -> 0x08, 0x3d2 -> 0x20,
 * 0x3d3 -> 0x80. The register OFFSET itself did not survive. INFERRED.
 */
#define CE5300_IMRX			0x0008	/* INFERRED -- verify before enabling IRQs */
#define CE5300_IRQ_MASK_TX0		0x08
#define CE5300_IRQ_MASK_TX1		0x20
#define CE5300_IRQ_MASK_TX2		0x80

/*
 * The descriptor the hardware walks. Exact, from DWARF -- not inferred.
 *
 * The allocation stride is 0x20, NOT sizeof(this): the vendor allocates a
 * 32-byte node (descriptor + 12 bytes of driver bookkeeping) and the setup loop
 * chains NEXT_DESC at 32-byte steps, with the descriptor memory aligned to 32.
 * The hardware may well require that alignment, so keep the stride even though
 * the last 12 bytes are ours.
 */
struct ce5300_dma_desc {
	__le32 next_desc;	/* +0x00 phys addr of the next descriptor; tail -> head */
	__le32 src_size;	/* +0x04 bytes */
	__le32 src_start;	/* +0x08 source phys addr (our audio buffer) */
	__le32 dst_start;	/* +0x0c dest phys addr (the TX FIFO port, MMIO) */
	__le32 flags_mode;	/* +0x10 copy of the DMA control word */
} __packed;

#define CE5300_DESC_STRIDE		0x20

#endif /* _CE5300_I2S_H */
