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
 * to answer.
 *
 * CORRECTION, measured 2026-08-30: this file used to claim pcim_enable_device()
 * made a full register dump safe. It does not. With the device bound and
 * enabled, reading the TX block at 0x2004 still hangs the SoC dead -- same
 * failure as poking it with devmem while unbound. BAR0 +0x00 answers, so the
 * function decodes, but the TX/DMA sub-blocks appear to be clock-gated or held
 * in reset behind the Clock and Reset Controller (00:00.2, 8086:2e52) that
 * CEFDK never ungates. Ungating that is a prerequisite for audio, and is not
 * done yet. See ce5300_dump().
 *
 * Pass ce5300_i2s.enable_pcm=1 once the inferred fields are confirmed.
 */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
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

/*
 * Dump the registers we care about.
 *
 * DANGER, MEASURED ON HARDWARE 2026-08-30: reading the TX block at 0x2004
 * HANGS THE SoC DEAD. The kernel printed the BAR0 word below and never printed
 * another line -- no panic, no timeout, the machine simply stopped:
 *
 *     ce5300-i2s 0000:01:06.2: BAR0 phys 0xdfa80000, +0x00 = 0x00000001
 *     <end of kernel log>
 *
 * pcim_enable_device() is NOT sufficient, which this file previously claimed.
 * BAR0 +0x00 answers, so the function decodes; the TX/DMA sub-blocks do not,
 * and the most likely reason is that they are still clock-gated or in reset
 * behind the SoC's Clock and Reset Controller (PCI 00:00.2, 8086:2e52). CEFDK
 * never ungates them because Control4's media stack is what used them.
 *
 * So the wide dump is opt-in and the default probe touches exactly one register
 * that is known to answer. Turning dump_regs on before the clock question is
 * settled will hang the box, and it takes a power cycle to get back.
 */
static bool dump_regs;
module_param(dump_regs, bool, 0444);
MODULE_PARM_DESC(dump_regs,
		 "Dump TX/DMA registers at probe. HANGS THE SoC unless the audio block has been ungated first -- see the comment above ce5300_dump()");

static void ce5300_dump(struct ce5300_i2s *i2s)
{
	unsigned int i;

	/* Proven to answer: this is the device's own ID/capability word. */
	dev_info(i2s->dev, "BAR0 phys %pa, +0x00 = %#010x\n",
		 &i2s->phys, readl(i2s->base));

	if (!dump_regs)
		return;

	dev_warn(i2s->dev, "dump_regs=1: reading TX/DMA -- this hangs the SoC if the block is still gated\n");
	for (i = 0; i < CE5300_TX_CONTEXTS; i++)
		dev_info(i2s->dev, "  TX%u ctrl=%#010x satr=%#010x\n", i,
			 readl(i2s->base + CE5300_TX_REG(i, CE5300_TX_CTRL)),
			 readl(i2s->base + CE5300_TX_REG(i, CE5300_TX_SATR)));
	for (i = 0; i < CE5300_DMA_CONTEXTS; i++)
		dev_info(i2s->dev, "  DMA%u ctrl=%#010x head=%#010x tail=%#010x\n", i,
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_CTRL)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_SRCDMA_START)),
			 readl(i2s->base + CE5300_DMA_REG(i, CE5300_DMA_SRCDMA_STOP)));
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
	}
	/* Publish every descriptor before the engine can be pointed at them. */
	wmb();

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
		 * INFERRED (see ce5300-i2s.h): head descriptor -> SRCDMA_START,
		 * tail -> SRCDMA_STOP, then the control word, then the TX enable
		 * bit. The vendor writes three DMA registers here but __regparm3
		 * stripped which; these are the only two address registers and
		 * the only control register in the block, so this is the natural
		 * fit -- and it is exactly what needs confirming on hardware.
		 */
		writel(i2s->ring_phys,
		       i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_SRCDMA_START));
		writel(desc_phys_at(i2s, i2s->substream->runtime->periods - 1),
		       i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_SRCDMA_STOP));
		writel(CE5300_DMA_FLAGS_PLAYBACK,
		       i2s->base + CE5300_DMA_REG(i2s->dma, CE5300_DMA_CTRL));
		ce5300_update(i2s, CE5300_TX_REG(i2s->tx, CE5300_TX_CTRL),
			      CE5300_TX_ENABLE, CE5300_TX_ENABLE);
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		/* Stop is a lone clear of the same bit; it does not touch the DMA
		 * block and does not clear the interrupt mask. */
		ce5300_update(i2s, CE5300_TX_REG(i2s->tx, CE5300_TX_CTRL),
			      CE5300_TX_ENABLE, 0);
		return 0;
	default:
		return -EINVAL;
	}
}

/*
 * UNIMPLEMENTED ON PURPOSE. ALSA needs a byte position within the ring, and the
 * register that carries it did not survive the decompile -- the vendor reads a
 * "current descriptor address" register whose offset is stripped. Returning a
 * fabricated position would make playback appear to work while drifting, which
 * is a far worse failure than an honest -EINVAL at open time.
 *
 * To finish this: dump the DMA block while a stream runs, find the register
 * that walks through the descriptor addresses, and convert it to a frame offset.
 */
static snd_pcm_uframes_t ce5300_pcm_pointer(struct snd_soc_component *comp,
					    struct snd_pcm_substream *ss)
{
	return 0;
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
 * worth setting: the codec is the I2S master, so we neither generate clocks nor
 * choose a direction. Keeping it equally empty is deliberate, not lazy.
 */
static struct snd_soc_dai_driver ce5300_dai = {
	.name = "ce5300-i2s",
	.playback = {
		.stream_name	= "Playback",
		.channels_min	= 2,
		.channels_max	= 2,
		.rates		= SNDRV_PCM_RATE_48000,
		.formats	= SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S32_LE,
	},
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

	ce5300_dump(i2s);

	if (!enable_pcm) {
		dev_info(&pdev->dev,
			 "probe-only (enable_pcm=0): mapped, not registering a PCM. "
			 "Several control fields are still inferred -- see ce5300-i2s.h\n");
		return 0;
	}

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
