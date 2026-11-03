// SPDX-License-Identifier: GPL-2.0
/*
 * ASoC platform (I2S + scatter-gather DMA) driver for the Intel CE5300.
 *
 * Binds PCI 8086:2e60 ("CE Media Processor Audio Interfaces"). This replaces
 * Control4's out-of-tree c4-smd-pcm-audio, which was never published -- the GPL
 * drop references it but does not contain it, so the register knowledge here
 * comes from the DWARF debug info left in ismdaudio.ko. See ce5300-i2s.h for
 * the map and, importantly, for which fields are proven and which are inferred.
 *
 * ce5300_i2s.enable_pcm=0 is the probe-only fallback: the CPU DAI behind a
 * component with no PCM ops and a startup that refuses, so the card and its
 * DSP controls still form while anything opening the PCM gets -ENODEV.
 *
 * The block itself comes up whatever enable_pcm says, because the I2S clocks are what the rest of the EA3's audio runs on
 * (ce5300_hw_start()). Reading the TX block used to hang the SoC dead (measured
 * 2026-08-30): CEFDK leaves the block clocked but held in its own reset,
 * +0x00 bit 1, and the TX/DMA sub-blocks do not answer until that is released
 * (measured 2026-11-02, with the release stock's driver does first).
 *
 * MEASURED 2026-10-26 on a stock unit while it played (ce5300-i2s.h): the base,
 * the descriptor format and mode word, the DMA context's registers (and with
 * them the playback position), and TX0's configuration. MEASURED 2026-11-03 on
 * an EA3, with DMA 0 driven by hand from a reserved buffer: stock's three start
 * writes run the engine into the TX0 FIFO, and STOP_AFTER halts it cleanly;
 * through ALSA, TX0's samples reach the line out via the DSP's SDATA_IN0.
 *
 * Three streams, one CPU DAI each, in the vendor's context map: the I2S out
 * (TX0/DMA0, the EA3's DSP and analog jacks), the S/PDIF out (TX2/DMA2, the
 * coax out) and the S/PDIF in (RX1/DMA4, the coax in; stock's hw_dev_id 0x4d3).
 *
 * Periods are paced by a timer, not the interrupt: the PCI interrupt is not
 * routed on every kernel openHC boots, and the position register is exact, so
 * a period-length hrtimer that reports elapsed periods from it is all ALSA
 * needs (the pattern of sound/drivers/dummy.c).
 */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/hrtimer.h>
#include <linux/io.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>

#include "ce5300-i2s.h"

#define DRV_NAME		"ce5300-i2s"
#define CE5300_PERIODS_MAX	32
#define CE5300_BUFFER_BYTES_MAX	(256 * 1024)

static bool enable_pcm = true;
module_param(enable_pcm, bool, 0444);
MODULE_PARM_DESC(enable_pcm,
		 "Register the PCM (0: the DAI and the DSP controls only, no playable PCM)");

enum { CE5300_I2S_OUT, CE5300_SPDIF_OUT, CE5300_SPDIF_IN, CE5300_STREAMS };

struct ce5300_i2s;

struct ce5300_stream {
	struct ce5300_i2s	*i2s;
	bool			capture;
	u32			ctx;		/* TX/RX context's register base */
	unsigned int		dma;		/* DMA context 0..4 */
	u32			fifo;		/* the FIFO port: TX_SATR or RX_SARR */
	u32			flags;		/* descriptor mode word, STOP_AFTER clear */

	struct ce5300_dma_desc	*ring;		/* CE5300_DESC_STRIDE-spaced, not packed */
	dma_addr_t		ring_phys;
	struct snd_pcm_substream *substream;

	struct hrtimer		tick;		/* one period; reports elapsed periods */
	ktime_t			period;
	atomic_t		running;
};

struct ce5300_i2s {
	struct device		*dev;
	void __iomem		*base;
	resource_size_t		phys;		/* BAR0 physical; descriptors point at MMIO by phys addr */
	struct ce5300_stream	st[CE5300_STREAMS];
};

/* The ring uses a 32-byte stride, so index it by bytes rather than by C array. */
static struct ce5300_dma_desc *desc_at(struct ce5300_stream *st, unsigned int i)
{
	return (struct ce5300_dma_desc *)((u8 *)st->ring + i * CE5300_DESC_STRIDE);
}

static dma_addr_t desc_phys_at(struct ce5300_stream *st, unsigned int i)
{
	return st->ring_phys + i * CE5300_DESC_STRIDE;
}

static void __iomem *dma_reg(struct ce5300_stream *st, u32 off)
{
	return st->i2s->base + CE5300_DMA_REG(st->dma, off);
}

static void ce5300_update(struct ce5300_i2s *i2s, u32 off, u32 mask, u32 val)
{
	u32 v = readl(i2s->base + off);

	v = (v & ~mask) | (val & mask);
	writel(v, i2s->base + off);
}

static bool clocks = true;
module_param(clocks, bool, 0444);
MODULE_PARM_DESC(clocks,
		 "Start TX0 at probe so the I2S clocks run, as stock does (the EA3's DSP and DAC have no clock otherwise)");

/*
 * A context's reset, the vendor's tx_reset/rx_reset: control word set, FIFO
 * reset pulsed (+0x00 bit 3), then bits of +0x10 cleared -- 0x120 for a TX
 * context, 0x100 for an RX one. TX and RX lay these out the same way. The
 * vendor writes 0 to the control word; the idle contexts of a playing stock
 * unit read 0x000e0000 (TX) and 0x001e0000 (RX) instead -- also their reset
 * values, with the DMA-context field at 7, none -- so that is what goes in,
 * rather than binding every idle context to DMA 0.
 */
static void ce5300_ctx_reset(struct ce5300_i2s *i2s, u32 ctx, u32 ctrl, u32 clear10)
{
	writel(ctrl, i2s->base + ctx + 0x04);
	ce5300_update(i2s, ctx, CE5300_CTX_FIFO_RESET, CE5300_CTX_FIFO_RESET);
	ce5300_update(i2s, ctx, CE5300_CTX_FIFO_RESET, 0);
	ce5300_update(i2s, ctx + 0x10, clear10, 0);
}

#define CE5300_PRIME_WORDS	32

/*
 * Start a TX context into its FIFO's silence: control word, the stock
 * registers, run, then a few zero words, without which it sends nothing. Once
 * running it sends on through underruns and ignores TX_CTRL writes.
 */
static void ce5300_tx_start(struct ce5300_i2s *i2s, struct ce5300_stream *st,
			    u32 ctrl, u32 r08, u32 r10, u32 r14, u32 r1c, u32 run)
{
	unsigned int i;

	writel(ctrl | st->dma << CE5300_TX_DMA_CTX_SHIFT, i2s->base + st->ctx + CE5300_TX_CTRL);
	if (r08)
		writel(r08, i2s->base + st->ctx + 0x08);
	if (r10)
		writel(r10, i2s->base + st->ctx + 0x10);
	writel(r14, i2s->base + st->ctx + 0x14);
	writel(r1c, i2s->base + st->ctx + 0x1c);
	writel(run, i2s->base + st->ctx + CE5300_TX_RUN_REG);
	for (i = 0; i < CE5300_PRIME_WORDS; i++)
		writel(0, i2s->base + st->fifo);
}

/*
 * Bring the block out of reset and, with clocks=1, start TX0 and TX2.
 *
 * The CE5300 is the bit- and frame-clock master of the EA3's I2S: every
 * serial port of the ADAU1451 is a slave, and the AK4621 behind it is
 * strapped standalone. Stock keeps its render pipeline running when nothing
 * plays, so those clocks never stop. Stopped, as CEFDK leaves them, the DSP
 * core sleeps waiting for a first frame and nothing reaches the jacks, not
 * even the DSP's own test tone. MEASURED 2026-11-02 on an EA3 (the DSP
 * README): with TX0 started this way the core runs, and the tone plays.
 *
 * The order is stock's: audio_ce53xx_hal_io_reset_hw (clock on, reset pulsed,
 * every DMA/TX/RX context reset, interrupts cleared and masked), then the
 * render path's pin and clock setup. TX0's own registers take the values a
 * playing stock unit holds. Its clocks start only once the FIFO has had data
 * and run on through underruns, so a few zero words prime it.
 *
 * TX2, the coax out, starts the same way, so the S/PDIF line carries a
 * stream from boot and a receiver locks once, not at every song.
 */
static void ce5300_hw_start(struct ce5300_i2s *i2s)
{
	unsigned int i;

	ce5300_update(i2s, CE5300_GLOBAL, CE5300_GLOBAL_CLK_EN, CE5300_GLOBAL_CLK_EN);
	ce5300_update(i2s, CE5300_GLOBAL, CE5300_GLOBAL_RUN, 0);
	ce5300_update(i2s, CE5300_GLOBAL, CE5300_GLOBAL_RUN, CE5300_GLOBAL_RUN);

	for (i = 0; i < CE5300_DMA_CONTEXTS; i++)
		writel(0, i2s->base + CE5300_DMA_REG(i, CE5300_DMA_CTRL));
	for (i = 0; i < CE5300_TX_CONTEXTS; i++)
		ce5300_ctx_reset(i2s, CE5300_TX_REG(i, 0), CE5300_TX_IDLE_04, 0x120);
	for (i = 0; i < CE5300_RX_CONTEXTS; i++)
		ce5300_ctx_reset(i2s, CE5300_RX_REG(i, 0), CE5300_RX_IDLE_04, 0x100);
	writel(CE5300_IRQ_STATUS_ALL, i2s->base + CE5300_IRQ_STATUS);
	writel(0, i2s->base + CE5300_IMRX);

	if (!clocks)
		return;

	ce5300_update(i2s, CE5300_GLOBAL, CE5300_GLOBAL_I2S0_OUT, CE5300_GLOBAL_I2S0_OUT);
	writel(CE5300_I2S_CFG_STOCK, i2s->base + CE5300_I2S_CFG);

	ce5300_tx_start(i2s, &i2s->st[CE5300_I2S_OUT], CE5300_TX0_STOCK_04,
			CE5300_TX0_STOCK_08, CE5300_TX0_STOCK_10, CE5300_TX0_STOCK_14,
			CE5300_TX0_STOCK_1C, CE5300_TX0_STOCK_00);
	ce5300_tx_start(i2s, &i2s->st[CE5300_SPDIF_OUT], CE5300_TX0_STOCK_04, 0, 0,
			CE5300_TX_SPDIF_14, CE5300_TX_SPDIF_1C,
			CE5300_TX_SEL_SPDIF << CE5300_CTX_SEL_SHIFT | CE5300_TX_RUN);

	dev_info(i2s->dev, "TX0 running: I2S clocks on; TX2 running: S/PDIF out\n");
}

/*
 * The coax in, as stock's program_rx_interface leaves RX1 for 0x4d3: input
 * select S/PDIF, bound to its DMA context, FIFO threshold 0x3f, and the
 * S/PDIF bit in +0x0c. MEASURED 2026-11-02 with the same writes and the FIFO
 * read by hand: coax-in samples arrive bit for bit, 24 bits in 31:8.
 */
static void ce5300_rx_setup(struct ce5300_stream *st)
{
	struct ce5300_i2s *i2s = st->i2s;

	/* An empty FIFO, so no stale words lead the stream; it resets the rest. */
	ce5300_update(i2s, st->ctx, CE5300_CTX_FIFO_RESET, CE5300_CTX_FIFO_RESET);
	ce5300_update(i2s, st->ctx, CE5300_CTX_FIFO_RESET, 0);
	ce5300_update(i2s, st->ctx, CE5300_CTX_SEL_MASK,
		      CE5300_RX_SEL_SPDIF << CE5300_CTX_SEL_SHIFT);
	ce5300_update(i2s, st->ctx + CE5300_RX_CTRL,
		      CE5300_TX_DMA_CTX_MASK << CE5300_TX_DMA_CTX_SHIFT,
		      st->dma << CE5300_TX_DMA_CTX_SHIFT);
	writel(0x3f, i2s->base + st->ctx + CE5300_RX_THRESH);
	ce5300_update(i2s, st->ctx + CE5300_RX_INT, CE5300_RX_INT_SPDIF, CE5300_RX_INT_SPDIF);
}

/*
 * Dump the registers we care about. Safe since ce5300_hw_start(): before the
 * block's reset is released, reading the TX block HANGS THE SoC DEAD (measured
 * 2026-08-30, the kernel log simply stopped after the BAR0 line).
 */
static bool dump_regs;
module_param(dump_regs, bool, 0444);
MODULE_PARM_DESC(dump_regs, "Dump TX/DMA registers at probe");

static void ce5300_dump(struct ce5300_i2s *i2s)
{
	unsigned int i;

	dev_info(i2s->dev, "BAR0 phys %pa, +0x00 = %#010x\n",
		 &i2s->phys, readl(i2s->base));

	if (!dump_regs)
		return;
	for (i = 0; i < CE5300_TX_CONTEXTS; i++)
		dev_info(i2s->dev, "  TX%u run=%#010x ctrl=%#010x level=%#010x\n", i,
			 readl(i2s->base + CE5300_TX_REG(i, CE5300_TX_RUN_REG)),
			 readl(i2s->base + CE5300_TX_REG(i, CE5300_TX_CTRL)),
			 readl(i2s->base + CE5300_TX_REG(i, CE5300_TX_LEVEL)));
	for (i = 0; i < CE5300_DMA_CONTEXTS; i++)
		dev_info(i2s->dev, "  DMA%u ctrl=%#010x desc=%#010x next=%#010x src=%#010x left=%#010x\n", i,
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_CTRL)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_CUR_DESC)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_NEXT_DESC)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_CUR_SRC)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_SIZE)));
}

/* ---- PCM ---------------------------------------------------------------- */

static const struct snd_pcm_hardware ce5300_pcm_hw = {
	.info			= SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID |
				  SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_BLOCK_TRANSFER,
	/* Stock's 24 bits in 32, MSB-aligned: TX0's format, fixed while it runs. */
	.formats		= SNDRV_PCM_FMTBIT_S32_LE,
	.rates			= SNDRV_PCM_RATE_48000,
	.rate_min		= 48000,
	.rate_max		= 48000,
	.channels_min		= 2,
	.channels_max		= 2,
	.buffer_bytes_max	= CE5300_BUFFER_BYTES_MAX,
	.period_bytes_min	= 1024,
	.period_bytes_max	= CE5300_BUFFER_BYTES_MAX / 2,
	.periods_min		= 2,
	.periods_max		= CE5300_PERIODS_MAX,
};

static struct ce5300_stream *ce5300_stream_of(struct snd_soc_component *comp,
					      struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);
	struct snd_soc_dai *dai = snd_soc_rtd_to_cpu(snd_soc_substream_to_rtd(ss), 0);

	return &i2s->st[dai->id];
}

static int ce5300_pcm_open(struct snd_soc_component *comp,
			   struct snd_pcm_substream *ss)
{
	struct ce5300_stream *st = ce5300_stream_of(comp, ss);

	snd_soc_set_runtime_hwparams(ss, &ce5300_pcm_hw);
	/* Whole bursts per period: 256 bytes out, 128 in (the mode words'). */
	snd_pcm_hw_constraint_step(ss->runtime, 0, SNDRV_PCM_HW_PARAM_PERIOD_BYTES,
				   st->capture ? 128 : 256);
	st->substream = ss;
	return 0;
}

static int ce5300_pcm_close(struct snd_soc_component *comp,
			    struct snd_pcm_substream *ss)
{
	ce5300_stream_of(comp, ss)->substream = NULL;
	return 0;
}

/*
 * Build the descriptor ring: one descriptor per ALSA period, chained head to
 * tail and tail back to head. The chain is built ONCE and never re-linked --
 * flow is controlled purely by the STOP bit (see ce5300-i2s.h). The FIFO end
 * is a fixed MMIO address, the buffer end walks: the TX FIFO as the
 * destination for playback, the RX FIFO as the source for capture.
 */
static int ce5300_pcm_prepare(struct snd_soc_component *comp,
			      struct snd_pcm_substream *ss)
{
	struct ce5300_stream *st = ce5300_stream_of(comp, ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	unsigned int periods = rt->periods;
	unsigned int period_bytes = snd_pcm_lib_period_bytes(ss);
	u32 fifo = st->i2s->phys + st->fifo;
	unsigned int i;

	if (periods > CE5300_PERIODS_MAX)
		return -EINVAL;
	/*
	 * A capture engine finishes its descriptor only while words arrive; with
	 * the coax in dead it still sits in one. Never rewrite a ring it walks.
	 */
	if (readl(dma_reg(st, CE5300_DMA_CTRL)) & CE5300_DMA_RUN)
		return -EBUSY;

	for (i = 0; i < periods; i++) {
		struct ce5300_dma_desc *d = desc_at(st, i);
		u32 buf = rt->dma_addr + i * period_bytes;

		d->next_desc = cpu_to_le32(desc_phys_at(st, (i + 1) % periods));
		d->src_start = cpu_to_le32(st->capture ? fifo : buf);
		d->src_size  = cpu_to_le32(period_bytes);
		d->dst_start = cpu_to_le32(st->capture ? buf : fifo);
		/*
		 * Cyclic: every descriptor continues into the next. The vendor
		 * leaves a STOP on the newest descriptor because it feeds discrete
		 * buffers on demand; a cyclic ALSA ring is always fully populated.
		 */
		d->flags_mode = cpu_to_le32(st->flags);
		d->tag   = 0;
		d->size2 = cpu_to_le32(period_bytes);
		d->self  = cpu_to_le32(desc_phys_at(st, i));
	}
	/* Publish every descriptor before the engine can be pointed at them. */
	wmb();

	st->period = ns_to_ktime(div_u64((u64)rt->period_size * NSEC_PER_SEC, rt->rate));

	/*
	 * Nothing to set on a TX context: ce5300_hw_start() started TX0 and TX2
	 * at probe in stock's 24-in-32 stereo format, and TX_CTRL ignores writes
	 * while a context runs (MEASURED), so the format is fixed.
	 */
	if (st->capture)
		ce5300_rx_setup(st);
	return 0;
}

static int ce5300_pcm_trigger(struct snd_soc_component *comp,
			      struct snd_pcm_substream *ss, int cmd)
{
	struct ce5300_stream *st = ce5300_stream_of(comp, ss);
	unsigned int i;

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		if (st->capture) {
			/*
			 * Stock's capture_start: the RX run bit first, then the RX
			 * FIFO as the source, the head as the next descriptor and
			 * the mode word.
			 */
			ce5300_update(st->i2s, st->ctx, CE5300_TX_RUN, CE5300_TX_RUN);
			writel(st->i2s->phys + st->fifo, dma_reg(st, CE5300_DMA_CUR_SRC));
		} else {
			/*
			 * Stock's render_start, register for register: the TX
			 * FIFO as the destination, the head as the next
			 * descriptor, then the mode word as the control word.
			 * MEASURED 2026-11-03: without the destination the
			 * engine fetches the head and stalls.
			 */
			writel(st->i2s->phys + st->fifo, dma_reg(st, CE5300_DMA_DST));
		}
		/* The engine sets CE5300_DMA_RUN itself. */
		writel(st->ring_phys, dma_reg(st, CE5300_DMA_NEXT_DESC));
		writel(st->flags, dma_reg(st, CE5300_DMA_CTRL));
		atomic_set(&st->running, 1);
		hrtimer_start(&st->tick, st->period, HRTIMER_MODE_REL_SOFT);
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
		atomic_set(&st->running, 0);
		if (!hrtimer_callback_running(&st->tick))
			hrtimer_try_to_cancel(&st->tick);
		/*
		 * Stop by flow control, never by a register: STOP_AFTER on every
		 * descriptor halts the engine at the end of the one it is in.
		 * MEASURED 2026-11-03: it finishes, clears CE5300_DMA_RUN and
		 * idles. Writing a busy context's registers hung the SoC, twice.
		 * The TX contexts keep running into silence, so the clocks and
		 * the S/PDIF line stay.
		 */
		for (i = 0; i < ss->runtime->periods; i++)
			desc_at(st, i)->flags_mode =
				cpu_to_le32(st->flags | CE5300_DMA_STOP_AFTER);
		wmb();
		return 0;
	default:
		return -EINVAL;
	}
}

/*
 * Wait out a tick that was mid-flight when trigger stopped the stream, then
 * for the engine to finish its last descriptor, so prepare never rewrites a
 * ring the engine is still walking. An RX context stops once its DMA has.
 */
static int ce5300_pcm_sync_stop(struct snd_soc_component *comp,
				struct snd_pcm_substream *ss)
{
	struct ce5300_stream *st = ce5300_stream_of(comp, ss);
	u32 ctrl;

	hrtimer_cancel(&st->tick);
	if (readl_poll_timeout(dma_reg(st, CE5300_DMA_CTRL), ctrl,
			       !(ctrl & CE5300_DMA_RUN), 1000,
			       2 * ktime_to_us(st->period) + 20000)) {
		dev_warn(st->i2s->dev, "DMA%u still running after stop (%#010x)%s\n",
			 st->dma, ctrl, st->capture ? ": no S/PDIF input" : "");
		return 0;
	}
	if (st->capture)
		ce5300_update(st->i2s, st->ctx, CE5300_TX_RUN, 0);
	return 0;
}

/* One period has gone by: let ALSA read the position and refill. */
static enum hrtimer_restart ce5300_pcm_tick(struct hrtimer *t)
{
	struct ce5300_stream *st = container_of(t, struct ce5300_stream, tick);

	if (!atomic_read(&st->running))
		return HRTIMER_NORESTART;
	/* May stop the stream (xrun, drain), which clears running. */
	snd_pcm_period_elapsed(st->substream);
	if (!atomic_read(&st->running))
		return HRTIMER_NORESTART;
	hrtimer_forward_now(t, st->period);
	return HRTIMER_RESTART;
}

/*
 * The position: the context's current buffer-side address (the source while
 * playing, MEASURED to walk through the buffer while stock plays; the
 * destination while capturing). Our periods are contiguous in one buffer, so
 * its offset from the start is the byte position in the ring.
 */
static snd_pcm_uframes_t ce5300_pcm_pointer(struct snd_soc_component *comp,
					    struct snd_pcm_substream *ss)
{
	struct ce5300_stream *st = ce5300_stream_of(comp, ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	u32 addr = readl(dma_reg(st, st->capture ? CE5300_DMA_DST : CE5300_DMA_CUR_SRC));
	u32 off = addr - (u32)rt->dma_addr;

	if (off >= snd_pcm_lib_buffer_bytes(ss))
		return 0;
	return bytes_to_frames(rt, off);
}

static int ce5300_pcm_new(struct snd_soc_component *comp,
				struct snd_soc_pcm_runtime *rtd)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);

	snd_pcm_set_managed_buffer_all(rtd->pcm, SNDRV_DMA_TYPE_DEV, i2s->dev,
				       CE5300_BUFFER_BYTES_MAX,
				       CE5300_BUFFER_BYTES_MAX);
	return 0;
}

static const struct snd_soc_component_driver ce5300_component = {
	.name		= DRV_NAME,
	.open		= ce5300_pcm_open,
	.close		= ce5300_pcm_close,
	.prepare	= ce5300_pcm_prepare,
	.trigger	= ce5300_pcm_trigger,
	.sync_stop	= ce5300_pcm_sync_stop,
	.pointer	= ce5300_pcm_pointer,
	/* .pcm_new, not .pcm_construct: mainline renamed this member in 5.6,
	 * but this kernel's snd_soc_component_driver still calls it pcm_new.
	 * Same signature -- int (*)(struct snd_soc_component *,
	 * struct snd_soc_pcm_runtime *) -- so only the name changes. Every
	 * other member this driver sets exists here under its usual name. */
	.pcm_new	= ce5300_pcm_new,
};

/*
 * The CPU DAIs, one per stream, their ids the stream index. The vendor's
 * equivalent is a pure stub -- every callback is a debug print and a return
 * 0 -- because the CE5300 has no per-DAI state worth setting: it is the I2S
 * master, its outputs always drive their clocks (the HAL has no render
 * master/slave bit), and probe has already started them. Keeping them equally
 * empty is deliberate, not lazy.
 */
#define CE5300_DAI_STREAM(name) {					\
	.stream_name	= name,						\
	.channels_min	= 2,						\
	.channels_max	= 2,						\
	.rates		= SNDRV_PCM_RATE_48000,				\
	.formats	= SNDRV_PCM_FMTBIT_S32_LE,			\
}

static struct snd_soc_dai_driver ce5300_dais[CE5300_STREAMS] = {
	[CE5300_I2S_OUT]   = { .name = "ce5300-i2s", .id = CE5300_I2S_OUT,
			       .playback = CE5300_DAI_STREAM("I2S Playback") },
	[CE5300_SPDIF_OUT] = { .name = "ce5300-spdif-out", .id = CE5300_SPDIF_OUT,
			       .playback = CE5300_DAI_STREAM("S/PDIF Playback") },
	[CE5300_SPDIF_IN]  = { .name = "ce5300-spdif-in", .id = CE5300_SPDIF_IN,
			       .capture = CE5300_DAI_STREAM("S/PDIF Capture") },
};

/* ---- probe-only: the DAI without a PCM ---------------------------------- */

static int ce5300_dai_startup_off(struct snd_pcm_substream *ss,
				  struct snd_soc_dai *dai)
{
	dev_dbg(dai->dev, "PCM not registered (ce5300_i2s.enable_pcm=0)\n");
	return -ENODEV;
}

static const struct snd_soc_dai_ops ce5300_dai_off_ops = {
	.startup = ce5300_dai_startup_off,
};

static struct snd_soc_dai_driver ce5300_dais_off[CE5300_STREAMS] = {
	[CE5300_I2S_OUT]   = { .name = "ce5300-i2s", .id = CE5300_I2S_OUT,
			       .playback = CE5300_DAI_STREAM("I2S Playback"),
			       .ops = &ce5300_dai_off_ops },
	[CE5300_SPDIF_OUT] = { .name = "ce5300-spdif-out", .id = CE5300_SPDIF_OUT,
			       .playback = CE5300_DAI_STREAM("S/PDIF Playback"),
			       .ops = &ce5300_dai_off_ops },
	[CE5300_SPDIF_IN]  = { .name = "ce5300-spdif-in", .id = CE5300_SPDIF_IN,
			       .capture = CE5300_DAI_STREAM("S/PDIF Capture"),
			       .ops = &ce5300_dai_off_ops },
};

static const struct snd_soc_component_driver ce5300_component_off = {
	.name = DRV_NAME,
};

/* ---- PCI --------------------------------------------------------------- */

/* The vendor's context map: hw_dev_id 0x3d1 (TX0/DMA0), 0x3d3 (TX2/DMA2), 0x4d3 (RX1/DMA4). */
static const struct {
	bool capture;
	u32 ctx, fifo;
	unsigned int dma;
} ce5300_streams[CE5300_STREAMS] = {
	[CE5300_I2S_OUT]   = { false, CE5300_TX_REG(0, 0), CE5300_TX_REG(0, CE5300_TX_SATR), 0 },
	[CE5300_SPDIF_OUT] = { false, CE5300_TX_REG(2, 0), CE5300_TX_REG(2, CE5300_TX_SATR), 2 },
	[CE5300_SPDIF_IN]  = { true,  CE5300_RX_REG(1, 0), CE5300_RX_REG(1, CE5300_RX_SARR), 4 },
};

static int ce5300_i2s_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct ce5300_i2s *i2s;
	unsigned int i;
	int ret;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;

	i2s = devm_kzalloc(&pdev->dev, sizeof(*i2s), GFP_KERNEL);
	if (!i2s)
		return -ENOMEM;

	i2s->dev = &pdev->dev;
	i2s->base = pcim_iomap(pdev, 0, 0);
	if (!i2s->base)
		return -ENOMEM;
	i2s->phys = pci_resource_start(pdev, 0);

	for (i = 0; i < CE5300_STREAMS; i++) {
		struct ce5300_stream *st = &i2s->st[i];

		st->i2s = i2s;
		st->capture = ce5300_streams[i].capture;
		st->ctx = ce5300_streams[i].ctx;
		st->fifo = ce5300_streams[i].fifo;
		st->dma = ce5300_streams[i].dma;
		st->flags = st->capture ? CE5300_DMA_FLAGS_CAPTURE : CE5300_DMA_FLAGS_PLAYBACK;
	}
	pci_set_master(pdev);
	dev_set_drvdata(&pdev->dev, i2s);

	ce5300_hw_start(i2s);
	ce5300_dump(i2s);

	if (!enable_pcm) {
		dev_info(&pdev->dev,
			 "probe-only (enable_pcm=0): DAIs registered without a PCM\n");
		return devm_snd_soc_register_component(&pdev->dev,
						       &ce5300_component_off,
						       ce5300_dais_off, CE5300_STREAMS);
	}

	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;

	for (i = 0; i < CE5300_STREAMS; i++) {
		struct ce5300_stream *st = &i2s->st[i];

		hrtimer_setup(&st->tick, ce5300_pcm_tick, CLOCK_MONOTONIC,
			      HRTIMER_MODE_REL_SOFT);
		st->ring = dmam_alloc_coherent(&pdev->dev,
					       CE5300_PERIODS_MAX * CE5300_DESC_STRIDE,
					       &st->ring_phys, GFP_KERNEL);
		if (!st->ring)
			return -ENOMEM;
		/* The engine chases 32-byte-granular next pointers; the vendor aligns
		 * the descriptor memory the same way, so refuse rather than misbehave. */
		if (st->ring_phys & (CE5300_DESC_STRIDE - 1)) {
			dev_err(&pdev->dev, "descriptor ring not %d-byte aligned\n",
				CE5300_DESC_STRIDE);
			return -EINVAL;
		}
	}

	ret = devm_snd_soc_register_component(&pdev->dev, &ce5300_component,
					      ce5300_dais, CE5300_STREAMS);
	if (ret)
		return ret;

	dev_info(&pdev->dev, "CE5300 I2S: TX0/DMA0 I2S out, TX2/DMA2 S/PDIF out, RX1/DMA4 S/PDIF in\n");
	return 0;
}

static const struct pci_device_id ce5300_i2s_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x2e60) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce5300_i2s_ids);

static struct pci_driver ce5300_i2s_driver = {
	.name		= DRV_NAME,
	.id_table	= ce5300_i2s_ids,
	.probe		= ce5300_i2s_probe,
};
module_pci_driver(ce5300_i2s_driver);

MODULE_DESCRIPTION("Intel CE5300 I2S/DMA ASoC platform driver");
MODULE_LICENSE("GPL");
