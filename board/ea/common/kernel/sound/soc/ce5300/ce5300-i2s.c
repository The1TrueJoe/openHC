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
 * WHY THIS DOES NOT REGISTER A PCM BY DEFAULT
 * ------------------------------------------
 * Four things could not be recovered from the decompile: which three DMA
 * registers the vendor's render_start writes, which bit of TX_CTRL is the
 * stream enable, the interrupt mask register's offset, and the DMA position
 * register. Registering a PCM whose trigger cannot reliably move samples is
 * worse than not registering one -- userspace opens it, gets silence, and the
 * failure looks like a codec or routing bug somewhere else entirely.
 *
 * So the default is probe-only: bind, map, and read ONE register that is known
 * to answer. It still registers the CPU DAI, behind a component with no PCM
 * ops and a startup that refuses: the board's simple-audio-card then forms the
 * card, so the codec's DSP controls exist and can be tuned, while anything
 * opening the PCM gets -ENODEV at once rather than reaching a TX register.
 *
 * The block itself does come up, and probe brings it up whatever enable_pcm
 * says, because the I2S clocks are what the rest of the EA3's audio runs on
 * (ce5300_hw_start()). Reading the TX block used to hang the SoC dead (measured
 * 2026-08-30): CEFDK leaves the block clocked but held in its own reset,
 * +0x00 bit 1, and the TX/DMA sub-blocks do not answer until that is released
 * (measured 2026-11-02, with the release stock's driver does first).
 *
 * Pass ce5300_i2s.enable_pcm=1 once the inferred fields are confirmed.
 *
 * MEASURED 2026-10-26 on a stock unit while it played (ce5300-i2s.h): the base,
 * the descriptor format and mode word, the DMA context's registers (and with
 * them the playback position), and TX0's configuration. What remains inferred
 * is which writes START the engine, and the ungate itself.
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
#include <linux/module.h>
#include <linux/pci.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>

#include "ce5300-i2s.h"

#define DRV_NAME		"ce5300-i2s"
#define CE5300_PERIODS_MAX	32
#define CE5300_BUFFER_BYTES_MAX	(256 * 1024)

static bool enable_pcm;
module_param(enable_pcm, bool, 0444);
MODULE_PARM_DESC(enable_pcm,
		 "Register the PCM. Off by default: several control-register fields are still inferred, see ce5300-i2s.h");

struct ce5300_i2s {
	struct device		*dev;
	void __iomem		*base;
	resource_size_t		phys;		/* BAR0 physical; descriptors point at MMIO by phys addr */
	unsigned int		tx;		/* TX context 0..2  */
	unsigned int		dma;		/* DMA context 0..4 */

	struct ce5300_dma_desc	*ring;		/* CE5300_DESC_STRIDE-spaced, not packed */
	dma_addr_t		ring_phys;
	unsigned int		nr_desc;
	struct snd_pcm_substream *substream;

	struct hrtimer		tick;		/* one period; reports elapsed periods */
	ktime_t			period;
	atomic_t		running;
};

/* The ring uses a 32-byte stride, so index it by bytes rather than by C array. */
static struct ce5300_dma_desc *desc_at(struct ce5300_i2s *i2s, unsigned int i)
{
	return (struct ce5300_dma_desc *)((u8 *)i2s->ring + i * CE5300_DESC_STRIDE);
}

static dma_addr_t desc_phys_at(struct ce5300_i2s *i2s, unsigned int i)
{
	return i2s->ring_phys + i * CE5300_DESC_STRIDE;
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

/*
 * Bring the block out of reset and, with clocks=1, start TX0.
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
 */
#define CE5300_PRIME_WORDS	32

static void ce5300_hw_start(struct ce5300_i2s *i2s)
{
	u32 tx = CE5300_TX_REG(i2s->tx, 0);
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

	writel(CE5300_TX0_STOCK_04, i2s->base + tx + CE5300_TX_CTRL);
	writel(CE5300_TX0_STOCK_08, i2s->base + tx + 0x08);
	writel(CE5300_TX0_STOCK_10, i2s->base + tx + 0x10);
	writel(CE5300_TX0_STOCK_14, i2s->base + tx + 0x14);
	writel(CE5300_TX0_STOCK_1C, i2s->base + tx + 0x1c);
	writel(CE5300_TX0_STOCK_00, i2s->base + tx + CE5300_TX_RUN_REG);
	for (i = 0; i < CE5300_PRIME_WORDS; i++)
		writel(0, i2s->base + tx + CE5300_TX_SATR);

	dev_info(i2s->dev, "TX%u running: I2S clocks on\n", i2s->tx);
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
	.formats		= SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S32_LE,
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

static int ce5300_pcm_open(struct snd_soc_component *comp,
			   struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);

	snd_soc_set_runtime_hwparams(ss, &ce5300_pcm_hw);
	i2s->substream = ss;
	return 0;
}

static int ce5300_pcm_close(struct snd_soc_component *comp,
			    struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);

	i2s->substream = NULL;
	return 0;
}

/*
 * Build the descriptor ring: one descriptor per ALSA period, chained head to
 * tail and tail back to head. The chain is built ONCE and never re-linked --
 * flow is controlled purely by the STOP bit (see ce5300-i2s.h). The DMA
 * destination is a fixed MMIO address, the TX FIFO write port, which is why the
 * destination address mode is FIXED_CONTINOUS while the source is LINEAR.
 */
static int ce5300_pcm_prepare(struct snd_soc_component *comp,
			      struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);
	struct snd_pcm_runtime *rt = ss->runtime;
	unsigned int periods = rt->periods;
	unsigned int period_bytes = snd_pcm_lib_period_bytes(ss);
	u32 satr = i2s->phys + CE5300_TX_REG(i2s->tx, CE5300_TX_SATR);
	unsigned int i;

	if (periods > i2s->nr_desc)
		return -EINVAL;

	for (i = 0; i < periods; i++) {
		struct ce5300_dma_desc *d = desc_at(i2s, i);
		unsigned int next = (i + 1) % periods;

		d->next_desc = cpu_to_le32(desc_phys_at(i2s, next));
		d->src_start = cpu_to_le32(rt->dma_addr + i * period_bytes);
		d->src_size  = cpu_to_le32(period_bytes);
		d->dst_start = cpu_to_le32(satr);
		/*
		 * Cyclic playback: every descriptor continues into the next.
		 * The vendor leaves a STOP on the newest descriptor because it
		 * feeds discrete buffers on demand; a cyclic ALSA ring is always
		 * fully populated, so nothing should ever halt the engine.
		 */
		d->flags_mode = cpu_to_le32(CE5300_DMA_FLAGS_PLAYBACK);
		d->tag   = 0;
		d->size2 = cpu_to_le32(period_bytes);
		d->self  = cpu_to_le32(desc_phys_at(i2s, i));
	}
	/* Publish every descriptor before the engine can be pointed at them. */
	wmb();

	i2s->period = ns_to_ktime(div_u64((u64)rt->period_size * NSEC_PER_SEC, rt->rate));

	/* TX0's other registers as a playing stock unit has them (ce5300-i2s.h),
	 * the run bit included: TX0 runs from probe on, because the DSP and DAC
	 * take their clocks from it (ce5300_hw_start()), and only DMA starts and
	 * stops with the stream. */
	writel(CE5300_TX0_STOCK_00,
	       i2s->base + CE5300_TX_REG(i2s->tx, CE5300_TX_RUN_REG));
	writel(CE5300_TX0_STOCK_08, i2s->base + CE5300_TX_REG(i2s->tx, 0x08));
	writel(CE5300_TX0_STOCK_10, i2s->base + CE5300_TX_REG(i2s->tx, 0x10));
	writel(CE5300_TX0_STOCK_14, i2s->base + CE5300_TX_REG(i2s->tx, 0x14));
	writel(CE5300_TX0_STOCK_1C, i2s->base + CE5300_TX_REG(i2s->tx, 0x1c));

	/* Bind this TX stream to our DMA context, and set the I2S format. */
	ce5300_update(i2s, CE5300_TX_REG(i2s->tx, CE5300_TX_CTRL),
		      CE5300_TX_DMA_CTX_MASK << CE5300_TX_DMA_CTX_SHIFT,
		      i2s->dma << CE5300_TX_DMA_CTX_SHIFT);

	/* Stereo on one data line: pin0 enabled, pins 1-3 disabled. */
	ce5300_update(i2s, CE5300_TX_REG(i2s->tx, CE5300_TX_CTRL),
		      (CE5300_TX_PIN_MASK << CE5300_TX_PIN0_SHIFT) |
		      (CE5300_TX_PIN_MASK << CE5300_TX_PIN1_SHIFT) |
		      (CE5300_TX_PIN_MASK << CE5300_TX_PIN2_SHIFT) |
		      (CE5300_TX_PIN_MASK << CE5300_TX_PIN3_SHIFT) |
		      (0x3 << CE5300_TX_STORAGE_SHIFT) |
		      CE5300_TX_SAMPLE_24BIT,
		      (CE5300_PIN_ENABLE  << CE5300_TX_PIN0_SHIFT) |
		      (CE5300_PIN_DISABLE << CE5300_TX_PIN1_SHIFT) |
		      (CE5300_PIN_DISABLE << CE5300_TX_PIN2_SHIFT) |
		      (CE5300_PIN_DISABLE << CE5300_TX_PIN3_SHIFT) |
		      (CE5300_STORAGE_STEREO << CE5300_TX_STORAGE_SHIFT) |
		      (rt->format == SNDRV_PCM_FORMAT_S16_LE ? 0 : CE5300_TX_SAMPLE_24BIT));
	return 0;
}

static int ce5300_pcm_trigger(struct snd_soc_component *comp,
			      struct snd_pcm_substream *ss, int cmd)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
	case SNDRV_PCM_TRIGGER_RESUME:
	case SNDRV_PCM_TRIGGER_PAUSE_RELEASE:
		/*
		 * The vendor's three writes -- "descriptor base, size, mode" --
		 * onto the registers a playing context was MEASURED to hold: the
		 * head as the NEXT descriptor, nothing left in the current one,
		 * then the mode word with the run bit. Which of these starts the
		 * engine is still INFERRED; this is the reading that fits.
		 */
		writel(i2s->ring_phys,
		       i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_NEXT_DESC));
		writel(0, i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_SIZE));
		writel(CE5300_DMA_FLAGS_PLAYBACK | CE5300_DMA_RUN,
		       i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_CTRL));
		ce5300_update(i2s, CE5300_TX_REG(i2s->tx, CE5300_TX_RUN_REG),
			      CE5300_TX_RUN, CE5300_TX_RUN);
		atomic_set(&i2s->running, 1);
		hrtimer_start(&i2s->tick, i2s->period, HRTIMER_MODE_REL_SOFT);
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		atomic_set(&i2s->running, 0);
		if (!hrtimer_callback_running(&i2s->tick))
			hrtimer_try_to_cancel(&i2s->tick);
		/* TX0 keeps running, underrunning into silence: the clocks stay. */
		ce5300_update(i2s, CE5300_DMA_REG(i2s->dma, CE5300_DMA_CTRL),
			      CE5300_DMA_RUN, 0);
		return 0;
	default:
		return -EINVAL;
	}
}

/* Wait out a tick that was mid-flight when trigger stopped the stream. */
static int ce5300_pcm_sync_stop(struct snd_soc_component *comp,
				struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);

	hrtimer_cancel(&i2s->tick);
	return 0;
}

/* One period has gone by: let ALSA read the position and refill. */
static enum hrtimer_restart ce5300_pcm_tick(struct hrtimer *t)
{
	struct ce5300_i2s *i2s = container_of(t, struct ce5300_i2s, tick);

	if (!atomic_read(&i2s->running))
		return HRTIMER_NORESTART;
	/* May stop the stream (xrun, drain), which clears running. */
	snd_pcm_period_elapsed(i2s->substream);
	if (!atomic_read(&i2s->running))
		return HRTIMER_NORESTART;
	hrtimer_forward_now(t, i2s->period);
	return HRTIMER_RESTART;
}

/*
 * The position, from the context's current source address (MEASURED to walk
 * through the buffer while stock plays). Our periods are contiguous in one
 * buffer, so its offset from the start is the byte position in the ring.
 */
static snd_pcm_uframes_t ce5300_pcm_pointer(struct snd_soc_component *comp,
					    struct snd_pcm_substream *ss)
{
	struct ce5300_i2s *i2s = snd_soc_component_get_drvdata(comp);
	struct snd_pcm_runtime *rt = ss->runtime;
	u32 src = readl(i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_CUR_SRC));
	u32 off = src - (u32)rt->dma_addr;

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
 * The CPU DAI. The vendor's equivalent is a pure stub -- every callback is a
 * debug print and a return 0 -- because the CE5300 I2S has no per-DAI state
 * worth setting: it is the I2S master, its outputs always drive their clocks
 * (the HAL has no render master/slave bit), and probe has already started
 * them. Keeping it equally empty is deliberate, not lazy.
 */
#define CE5300_DAI_PLAYBACK {						\
	.stream_name	= "Playback",					\
	.channels_min	= 2,						\
	.channels_max	= 2,						\
	.rates		= SNDRV_PCM_RATE_48000,				\
	.formats	= SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S32_LE,	\
}

static struct snd_soc_dai_driver ce5300_dai = {
	.name = "ce5300-i2s",
	.playback = CE5300_DAI_PLAYBACK,
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

static struct snd_soc_dai_driver ce5300_dai_off = {
	.name = "ce5300-i2s",
	.playback = CE5300_DAI_PLAYBACK,
	.ops = &ce5300_dai_off_ops,
};

static const struct snd_soc_component_driver ce5300_component_off = {
	.name = DRV_NAME,
};

/* ---- PCI --------------------------------------------------------------- */

static int ce5300_i2s_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct ce5300_i2s *i2s;
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

	/*
	 * hw_dev_id 0x3d1 -> TX0/DMA0 is the first render stream in the vendor
	 * mapping; the EA3 routes its analog output through it.
	 */
	i2s->tx = 0;
	i2s->dma = 0;
	pci_set_master(pdev);
	dev_set_drvdata(&pdev->dev, i2s);

	ce5300_hw_start(i2s);
	ce5300_dump(i2s);

	if (!enable_pcm) {
		dev_info(&pdev->dev,
			 "probe-only (enable_pcm=0): mapped, DAI registered without a PCM. "
			 "Several control fields are still inferred -- see ce5300-i2s.h\n");
		return devm_snd_soc_register_component(&pdev->dev,
						       &ce5300_component_off,
						       &ce5300_dai_off, 1);
	}

	hrtimer_setup(&i2s->tick, ce5300_pcm_tick, CLOCK_MONOTONIC, HRTIMER_MODE_REL_SOFT);

	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;

	i2s->nr_desc = CE5300_PERIODS_MAX;
	i2s->ring = dmam_alloc_coherent(&pdev->dev,
					i2s->nr_desc * CE5300_DESC_STRIDE,
					&i2s->ring_phys, GFP_KERNEL);
	if (!i2s->ring)
		return -ENOMEM;
	/* The engine chases 32-byte-granular next pointers; the vendor aligns the
	 * descriptor memory the same way, so refuse rather than misbehave. */
	if (i2s->ring_phys & (CE5300_DESC_STRIDE - 1)) {
		dev_err(&pdev->dev, "descriptor ring not %d-byte aligned\n",
			CE5300_DESC_STRIDE);
		return -EINVAL;
	}

	ret = devm_snd_soc_register_component(&pdev->dev, &ce5300_component,
					      &ce5300_dai, 1);
	if (ret)
		return ret;

	dev_info(&pdev->dev, "CE5300 I2S: TX%u/DMA%u, %u descriptors\n",
		 i2s->tx, i2s->dma, i2s->nr_desc);
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
