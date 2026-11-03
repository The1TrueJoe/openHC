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
 * MEASURED marks what was read off a stock EA3 while it played (2026-10-26,
 * stock's ismdaudio driving the block; site/.../ea/audio-regmap.md has the
 * full capture). Measured beats inferred wherever they disagreed.
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

/* ---- Global block ------------------------------------------------------ */
/*
 * +0x00 reads 0x7 on a playing stock unit and 0x1 as CEFDK leaves it: the
 * block clocked but held in reset, which is why every TX/DMA access used to
 * hang the SoC. Stock's audio_ce53xx_hal_io_reset_hw sets the clock enable,
 * then pulses the reset bit clear-and-set before it touches anything at 0x1000
 * or above; io_alloc_resource later sets bit 2 for the I2S0 render stream.
 * MEASURED 2026-11-02 on an EA3: with bit 1 set, +0x2000 and the rest read
 * their reset values instead of hanging.
 */
#define CE5300_GLOBAL			0x0000
#define CE5300_GLOBAL_CLK_EN		BIT(0)	/* audio_hal_io_clock_enable */
#define CE5300_GLOBAL_RUN		BIT(1)	/* 0 = sub-blocks held in reset */
#define CE5300_GLOBAL_I2S0_OUT		BIT(2)	/* the shared I2S0 pins to TX0 */
#define CE5300_GLOBAL_STOCK		0x7	/* MEASURED, stock while playing */

/* Interrupt status, write 1 to clear; reset_hw clears all of it. */
#define CE5300_IRQ_STATUS		0x0004
#define CE5300_IRQ_STATUS_ALL		0xe807f8

/*
 * +0x2c: bits 8 and 7 invert the I2S0/I2S1 bit clocks (stock sets both, so
 * 0x180), bit 10 picks the word-select edge; bits 4:2 and 0 are what
 * render/capture_set_power_mode set and clear (0 at reset, and TX0 runs with
 * them 0). From the audio_hal_*_set_* callers; stock reads 0x180.
 */
#define CE5300_I2S_CFG			0x002c
#define CE5300_I2S_CFG_STOCK		0x180

/* ---- DMA block: base 0x1000, 5 contexts, stride 0x40 ------------------- */
#define CE5300_DMA_BASE			0x1000
#define CE5300_DMA_STRIDE		0x40
#define CE5300_DMA_CONTEXTS		5
#define CE5300_DMA_REG(dma, off)	(CE5300_DMA_BASE + (dma) * CE5300_DMA_STRIDE + (off))

/*
 * MEASURED: the context's registers hold the descriptor it is working on, in
 * this order. On a playing stock unit: CUR_DESC and NEXT_DESC 0x20 apart in
 * the ring, CUR_SRC walking through the buffer, DST the TX0 FIFO port, SIZE
 * the bytes left (0xd00 of a 0xf00 descriptor with CUR_SRC 0x200 in), CTRL
 * the descriptor's mode word with CE5300_DMA_RUN set. 0x38 (once guessed to be
 * a tail register) reads 0 while it runs.
 */
#define CE5300_DMA_CUR_DESC		0x08
#define CE5300_DMA_NEXT_DESC		0x0c	/* start: the head descriptor goes here */
#define CE5300_DMA_CUR_SRC		0x10	/* the playback position */
#define CE5300_DMA_DST			0x14
#define CE5300_DMA_SIZE			0x18	/* bytes left; 0 = fetch NEXT_DESC */
#define CE5300_DMA_CTRL			0x1c	/* the flags/mode word; also FLAGS_MODE in a descriptor */

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
#define   CE5300_BURST_128		5	/* capture: the largest of 4..128 B dividing the buffer */
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
#define CE5300_DMA_BIT29		BIT(29)	/* set, and bit 30 clear, on the capture path */
/*
 * MEASURED: set in the CTRL register while stock plays, never in a descriptor's
 * mode word. The engine sets it when it starts and clears it when it halts on a
 * STOP_AFTER descriptor (2026-11-03); stock's render_start writes the mode word
 * without it, and so does this driver.
 */
#define CE5300_DMA_RUN			BIT(31)

/*
 * Playback flag word, minus the STOP bit. MEASURED: this is exactly the mode
 * word in every descriptor of a playing stock unit, 0x40006696.
 */
#define CE5300_DMA_FLAGS_PLAYBACK					\
	(CE5300_DMA_BIT30 |						\
	 (CE5300_BURST_256 << CE5300_DMA_BURST_SHIFT) |			\
	 (CE5300_BURST_256 << CE5300_DMA_XBURST_SHIFT) |		\
	 (CE5300_ADDR_FIXED_CONT << CE5300_DMA_DST_MODE_SHIFT) |	\
	 (CE5300_ADDR_LINEAR << CE5300_DMA_SRC_MODE_SHIFT) |		\
	 CE5300_DMA_FLAG_SRC_A | CE5300_DMA_FLAG_SRC_B)

/*
 * Capture flag word, minus the STOP bit: stock's capture_linked_list_dma_setup
 * tuples, render's mirrored -- the source (the RX FIFO) fixed, the destination
 * linear. 0x20005569 with 128-byte bursts, the size stock picks for a buffer
 * that is a multiple of 128 bytes.
 */
#define CE5300_DMA_FLAGS_CAPTURE					\
	(CE5300_DMA_BIT29 |						\
	 (CE5300_BURST_128 << CE5300_DMA_BURST_SHIFT) |			\
	 (CE5300_BURST_128 << CE5300_DMA_XBURST_SHIFT) |		\
	 (CE5300_ADDR_FIXED_CONT << CE5300_DMA_SRC_MODE_SHIFT) |	\
	 (CE5300_ADDR_LINEAR << CE5300_DMA_DST_MODE_SHIFT) |		\
	 CE5300_DMA_FLAG_DST_A | CE5300_DMA_FLAG_DST_B)

/* ---- TX block: base 0x2000, 3 contexts, stride 0x100 ------------------- */
#define CE5300_TX_BASE			0x2000
#define CE5300_TX_STRIDE		0x100
#define CE5300_TX_CONTEXTS		3
#define CE5300_TX_REG(tx, off)		(CE5300_TX_BASE + (tx) * CE5300_TX_STRIDE + (off))

#define CE5300_TX_CTRL			0x04
#define CE5300_TX_SATR			0x18	/* FIFO write port; the DMA destination */

/*
 * TX control. Ignores writes while the context runs (MEASURED 2026-11-03), so
 * it is set before the run bit. The I2S pin-enable fields are deliberately NOT in bit order --
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
#define CE5300_TX_SAMPLE_24BIT		BIT(7)	/* 0 = 16-bit; render_set_sample_size sets it for 24BIT */
#define CE5300_TX_DMA_CTX_SHIFT		17	/* [19:17] binds this TX stream to a DMA context */
#define CE5300_TX_DMA_CTX_MASK		0x7

/*
 * TX run. MEASURED: TX_CTRL bit 0 reads 0 on a playing stock unit, so it is
 * NOT the enable it was taken for. The context's +0x00 register reads 0x21
 * there; render_start's lone OrBitsReg32 on the TX block lands on bit 0 of it
 * (INFERRED: the bit, from the value; the register, from the measurement).
 */
#define CE5300_TX_RUN_REG		0x00
#define CE5300_TX_RUN			BIT(0)
/* Context +0x00 bit 3, TX and RX alike: pulsed by tx_reset/rx_reset. */
#define CE5300_CTX_FIFO_RESET		BIT(3)

/*
 * The rest of TX0 as a playing stock unit has it, MEASURED. Their fields are
 * unknown; prepare writes stock's values so the context is configured exactly
 * as it was when sound came out. TX_CTRL (0x04, 0xf1e0 there) is composed from
 * its decoded fields instead, and matches.
 */
#define CE5300_TX0_STOCK_00		0x21	/* incl. CE5300_TX_RUN */
#define CE5300_TX0_STOCK_04		0xf1e0	/* TX_CTRL: pin0, stereo, as decoded */
#define CE5300_TX0_STOCK_08		0x05
#define CE5300_TX0_STOCK_10		0x100
#define CE5300_TX0_STOCK_14		0x08	/* [6:0] bit-clock divider, MCLK / (64 x fs) */
#define CE5300_TX0_STOCK_1C		0x3f	/* FIFO threshold */
#define CE5300_TX_LEVEL			0x20	/* FIFO level, moving while it plays */
#define CE5300_TX_IDLE_04		0x000e0000	/* MEASURED: TX1/TX2 while stock plays; reset value */

/*
 * TX/RX +0x00 bits 6:5: the pins a context drives or reads. TX: AUDIO_HAL_OUTPUT_SEL
 * (I2S1 0, I2S0 1, S/PDIF 3); RX: AUDIO_HAL_INPUT_SEL (I2S1 0, I2S0 1, S/PDIF 2).
 * TX0's stock 0x21 is I2S0 and run.
 */
#define CE5300_CTX_SEL_SHIFT		5
#define CE5300_CTX_SEL_MASK		(0x3 << CE5300_CTX_SEL_SHIFT)
#define   CE5300_TX_SEL_SPDIF		3
#define   CE5300_RX_SEL_SPDIF		2

/*
 * TX2 as the coax out: TX0's format with output select S/PDIF and DMA 2.
 * MEASURED 2026-11-02 (EA3, coax out cabled to coax in): with these, words fed
 * to TX2's FIFO came back on RX1 bit for bit. +0x14 is half TX0's: S/PDIF's
 * biphase line runs at 128 fs, I2S's bit clock at 64 fs; at TX0's 8 the coax
 * out ran at exactly half TX0's frame rate (MEASURED 2026-11-03). Like
 * TX_CTRL it ignores writes while the context runs.
 */
#define CE5300_TX_SPDIF_14		0x04
#define CE5300_TX_SPDIF_1C		0x3f

/* ---- RX block: base 0x3000, 2 contexts, stride 0x100 ------------------- */
#define CE5300_RX_BASE			0x3000
#define CE5300_RX_STRIDE		0x100
#define CE5300_RX_CONTEXTS		2
#define CE5300_RX_REG(rx, off)		(CE5300_RX_BASE + (rx) * CE5300_RX_STRIDE + (off))
/*
 * The FIFO reset (+0x00 bit 3) puts +0x04, +0x0c and +0x1c back to their reset
 * values (MEASURED 2026-11-03), so they are set after it.
 */
#define CE5300_RX_CTRL			0x04	/* [19:17] the DMA context, as on TX */
#define CE5300_RX_INT			0x0c	/* stock ORs its sa_int_mask in at capture start */
#define   CE5300_RX_INT_SPDIF		0x200	/* 0x4d3, the S/PDIF input's */
#define CE5300_RX_SYNC			0x10	/* bit 0, write 1 to clear: no valid S/PDIF stream */
#define CE5300_RX_SARR			0x18	/* FIFO read port; the DMA source */
#define CE5300_RX_THRESH		0x1c	/* stock's rx_fifo_set_thrsh_level, 0x3f */
#define CE5300_RX_LEVEL			0x20
#define CE5300_RX_IDLE_04		0x001e0000	/* MEASURED: RX0 while stock plays; reset value */

/* ---- Interrupts -------------------------------------------------------- */
/*
 * render_start ends with a read-modify-write of an interrupt mask register
 * (the two locals survived in DWARF as imrx_val / new_imrx_val), OR-ing in a
 * per-stream mask chosen from hw_dev_id: 0x3d1 -> 0x08, 0x3d2 -> 0x20,
 * 0x3d3 -> 0x80. The register OFFSET itself did not survive. MEASURED: global
 * +0x0c reads exactly 0x08 while stock plays TX0/DMA0, the one place holding
 * that per-stream bit (+0x08, the old guess, reads 0). Not used: this driver
 * paces periods with a timer, because the PCI interrupt is not routed on every
 * kernel openHC boots (see ce5300_pcm_tick).
 */
#define CE5300_IMRX			0x000c
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
	/*
	 * The vendor's bookkeeping, MEASURED in a stock ring: a per-buffer word,
	 * the size again, and the descriptor's own address. The hardware's
	 * descriptor is the 20 bytes above; these are filled in the same way
	 * only so the ring looks exactly like stock's.
	 */
	__le32 tag;		/* +0x14 */
	__le32 size2;		/* +0x18 */
	__le32 self;		/* +0x1c */
} __packed;

#define CE5300_DESC_STRIDE		0x20

#endif /* _CE5300_I2S_H */
