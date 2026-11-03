// SPDX-License-Identifier: GPL-2.0
/*
 * Control4 EA-5 audio FPGA (Xilinx Spartan-6 on PCIe, 10ee:0007).
 *
 * The FPGA moves four stereo pairs each way between host memory and a 32x32
 * gain matrix that also connects the ADCs, the DACs, S/PDIF in/out and the two
 * ADAU1451s. Register map from Control4's GPL ninjago_fpga driver; see
 * site/src/content/docs/ea/ea5.md.
 *
 * Two drivers: the PCI one registers the component (a DAI per pair, the PCM,
 * the matrix controls), and the "control4,ea5-audio" node in the board DTB is
 * the card, with the DSPs as aux devices.
 */

#include <linux/debugfs.h>
#include <linux/dma-mapping.h>
#include <linux/interrupt.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/pci.h>
#include <linux/platform_device.h>
#include <linux/spinlock.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>
#include <sound/tlv.h>

#define DRV_NAME		"ea5-fpga"
#define EA5_FPGA_DEVICE		0x0007

/* BAR0 */
#define REG_REVISION		0x00
#define REG_STATUS		0x04
#define REG_CONTROL		0x08
#define  CONTROL_DAC_MUTE	BIT(17)
#define REG_BUFFER_CTRL		0x0c	/* 8 bits per pair: playback [3:0], capture [7:4] */
#define  BUF_SS_16		0x1
#define  BUF_SS_24		0x2
#define  BUF_SS_32		0x3
#define  BUF_SS_MASK		0x3
#define  BUF_START		BIT(2)
#define REG_IRQ_ENABLE		0x10	/* capture pair p = bit p, playback = bit p + 8 */
#define REG_IRQ_STATUS		0x14	/* write 1s to clear (FPGA rev <= 4) */
#define REG_IRQ_COUNT		0x30
#define REG_WDMA_TLP_SIZE	0x84	/* capture block, in dwords */
#define REG_WDMA_ADDR(p)	(0x90 + 4 * (p))
#define REG_RDMA_TLP_SIZE	0xa4	/* playback block, in dwords */
#define REG_RDMA_ADDR(p)	(0xb0 + 4 * (p))

#define NR_PAIRS		4
#define MATRIX_SIZE		32
#define MATRIX_UNITY		0x30f	/* 0 dB; 0.1 dB per step, 0 = -78.3 dB */
#define MATRIX_MAX		0x3ff	/* +24.0 dB */
#define MATRIX_REG(out, in)	((MATRIX_SIZE * (out) + (in)) * 2)

#define BUF_BYTES_MAX		(256 * 1024)

/* Matrix inputs (values) and outputs (controls). */
#define IN_PCM(p)		(2 * (p))	/* host playback pair p, L then R */
#define IN_ADC1			8
#define IN_ADC2			10
#define IN_SPDIF1		12
#define IN_SPDIF2		14
#define OUT_PCM(p)		(2 * (p))	/* host capture pair p */
#define OUT_DSP1_IN(n)		(8 + 2 * (n))
#define OUT_DSP2_IN(n)		(16 + 2 * (n))
#define OUT_DAC1		24
#define OUT_DAC2		26
#define OUT_SPDIF1		28
#define OUT_SPDIF2		30

/*
 * openHC's routing: each pair is one output and one input, the DSPs bypassed.
 * Pair 0 analog 1, 1 coax 1, 2 analog 2, 3 coax 2. The DSP inputs get what
 * Control4's routing gave them, so a DSP program can be switched in later.
 */
static const u8 ea5_default_routes[][2] = {	/* { out, in } stereo pairs */
	{ OUT_DAC1,       IN_PCM(0) },
	{ OUT_SPDIF1,     IN_PCM(1) },
	{ OUT_DAC2,       IN_PCM(2) },
	{ OUT_SPDIF2,     IN_PCM(3) },
	{ OUT_PCM(0),     IN_ADC1 },
	{ OUT_PCM(1),     IN_SPDIF1 },
	{ OUT_PCM(2),     IN_ADC2 },
	{ OUT_PCM(3),     IN_SPDIF2 },
	{ OUT_DSP1_IN(0), IN_ADC1 },
	{ OUT_DSP1_IN(1), IN_SPDIF1 },
	{ OUT_DSP1_IN(2), IN_PCM(0) },
	{ OUT_DSP1_IN(3), IN_PCM(1) },
	{ OUT_DSP2_IN(0), IN_ADC2 },
	{ OUT_DSP2_IN(1), IN_SPDIF2 },
	{ OUT_DSP2_IN(2), IN_PCM(2) },
	{ OUT_DSP2_IN(3), IN_PCM(3) },
};

struct ea5_stream {
	struct snd_pcm_substream *ss;
	unsigned int pos;		/* bytes the FPGA has moved, in the buffer */
	bool running;
};

struct ea5_fpga {
	struct pci_dev *pdev;
	void __iomem *regs;
	void __iomem *matrix;
	spinlock_t lock;		/* BUFFER_CTRL, IRQ_ENABLE, CONTROL, streams */
	u32 revision;
	bool irq_auto_clear;
	unsigned int blk;		/* bytes per DMA block, same both ways */
	int playing;			/* playback streams running: DACs unmuted */
	struct ea5_stream st[NR_PAIRS][2];
	struct snd_pcm_hardware hw;
	struct debugfs_regset32 regset;
};

static const struct debugfs_reg32 ea5_regs[] = {
	{ "revision", REG_REVISION },
	{ "status", REG_STATUS },
	{ "control", REG_CONTROL },
	{ "buffer_ctrl", REG_BUFFER_CTRL },
	{ "irq_enable", REG_IRQ_ENABLE },
	{ "irq_status", REG_IRQ_STATUS },
	{ "irq_count", REG_IRQ_COUNT },
	{ "wdma_tlp_size", REG_WDMA_TLP_SIZE },
	{ "wdma_addr0", REG_WDMA_ADDR(0) },
	{ "wdma_addr1", REG_WDMA_ADDR(1) },
	{ "wdma_addr2", REG_WDMA_ADDR(2) },
	{ "wdma_addr3", REG_WDMA_ADDR(3) },
	{ "rdma_tlp_size", REG_RDMA_TLP_SIZE },
	{ "rdma_addr0", REG_RDMA_ADDR(0) },
	{ "rdma_addr1", REG_RDMA_ADDR(1) },
	{ "rdma_addr2", REG_RDMA_ADDR(2) },
	{ "rdma_addr3", REG_RDMA_ADDR(3) },
};

static void ea5_update(struct ea5_fpga *f, u32 reg, u32 mask, u32 val)
{
	u32 v = readl(f->regs + reg);

	writel((v & ~mask) | (val & mask), f->regs + reg);
}

static unsigned int ea5_irq_bit(int pair, int dir)
{
	return dir == SNDRV_PCM_STREAM_PLAYBACK ? pair + 8 : pair;
}

static unsigned int ea5_buf_shift(int pair, int dir)
{
	return pair * 8 + (dir == SNDRV_PCM_STREAM_CAPTURE ? 4 : 0);
}

static void ea5_set_addr(struct ea5_fpga *f, int pair, int dir, dma_addr_t a)
{
	writel(lower_32_bits(a), f->regs + (dir == SNDRV_PCM_STREAM_PLAYBACK ?
		REG_RDMA_ADDR(pair) : REG_WDMA_ADDR(pair)));
}

/* ---- PCM ----------------------------------------------------------------- */

static int ea5_pair(struct snd_pcm_substream *ss)
{
	return snd_soc_rtd_to_cpu(snd_soc_substream_to_rtd(ss), 0)->id;
}

static int ea5_pcm_open(struct snd_soc_component *comp,
			struct snd_pcm_substream *ss)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);
	struct snd_pcm_runtime *rt = ss->runtime;
	int ret;

	snd_soc_set_runtime_hwparams(ss, &f->hw);
	/* The FPGA moves whole blocks, so periods and the buffer are made of them. */
	ret = snd_pcm_hw_constraint_step(rt, 0, SNDRV_PCM_HW_PARAM_PERIOD_BYTES, f->blk);
	if (ret)
		return ret;
	ret = snd_pcm_hw_constraint_step(rt, 0, SNDRV_PCM_HW_PARAM_BUFFER_BYTES, f->blk);
	if (ret)
		return ret;
	ret = snd_pcm_hw_constraint_integer(rt, SNDRV_PCM_HW_PARAM_PERIODS);
	if (ret < 0)
		return ret;

	f->st[ea5_pair(ss)][ss->stream].ss = ss;
	return 0;
}

static int ea5_pcm_close(struct snd_soc_component *comp,
			 struct snd_pcm_substream *ss)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);
	unsigned long flags;

	spin_lock_irqsave(&f->lock, flags);
	f->st[ea5_pair(ss)][ss->stream].ss = NULL;
	spin_unlock_irqrestore(&f->lock, flags);
	return 0;
}

static int ea5_pcm_hw_params(struct snd_soc_component *comp,
			     struct snd_pcm_substream *ss,
			     struct snd_pcm_hw_params *params)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);
	unsigned int shift = ea5_buf_shift(ea5_pair(ss), ss->stream);
	unsigned long flags;
	u32 ss_code;

	switch (params_physical_width(params)) {
	case 16: ss_code = BUF_SS_16; break;
	case 24: ss_code = BUF_SS_24; break;
	case 32: ss_code = BUF_SS_32; break;
	default: return -EINVAL;
	}

	spin_lock_irqsave(&f->lock, flags);
	ea5_update(f, REG_BUFFER_CTRL, BUF_SS_MASK << shift, ss_code << shift);
	spin_unlock_irqrestore(&f->lock, flags);
	return 0;
}

static int ea5_pcm_prepare(struct snd_soc_component *comp,
			   struct snd_pcm_substream *ss)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);
	int pair = ea5_pair(ss), dir = ss->stream;
	unsigned int bit = ea5_irq_bit(pair, dir);
	unsigned long flags;

	spin_lock_irqsave(&f->lock, flags);
	f->st[pair][dir].pos = 0;
	ea5_set_addr(f, pair, dir, ss->runtime->dma_addr);
	ea5_update(f, REG_IRQ_ENABLE, BIT(bit), BIT(bit));
	spin_unlock_irqrestore(&f->lock, flags);
	return 0;
}

static int ea5_pcm_trigger(struct snd_soc_component *comp,
			   struct snd_pcm_substream *ss, int cmd)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);
	int pair = ea5_pair(ss), dir = ss->stream;
	struct ea5_stream *s = &f->st[pair][dir];
	unsigned int shift = ea5_buf_shift(pair, dir);
	unsigned int bit = ea5_irq_bit(pair, dir);
	bool play = dir == SNDRV_PCM_STREAM_PLAYBACK;
	unsigned long flags;

	spin_lock_irqsave(&f->lock, flags);
	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
	case SNDRV_PCM_TRIGGER_RESUME:
	case SNDRV_PCM_TRIGGER_PAUSE_RELEASE:
		if (s->running)
			break;
		s->running = true;
		ea5_update(f, REG_IRQ_ENABLE, BIT(bit), BIT(bit));
		ea5_update(f, REG_BUFFER_CTRL, BUF_START << shift, BUF_START << shift);
		if (play && f->playing++ == 0)
			ea5_update(f, REG_CONTROL, CONTROL_DAC_MUTE, 0);
		break;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		if (!s->running)
			break;
		s->running = false;
		ea5_update(f, REG_IRQ_ENABLE, BIT(bit), 0);
		ea5_update(f, REG_BUFFER_CTRL, BUF_START << shift, 0);
		if (play && --f->playing == 0)
			ea5_update(f, REG_CONTROL, CONTROL_DAC_MUTE, CONTROL_DAC_MUTE);
		break;
	default:
		spin_unlock_irqrestore(&f->lock, flags);
		return -EINVAL;
	}
	spin_unlock_irqrestore(&f->lock, flags);
	return 0;
}

static snd_pcm_uframes_t ea5_pcm_pointer(struct snd_soc_component *comp,
					 struct snd_pcm_substream *ss)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);

	return bytes_to_frames(ss->runtime,
			       READ_ONCE(f->st[ea5_pair(ss)][ss->stream].pos));
}

static int ea5_pcm_new(struct snd_soc_component *comp,
		       struct snd_soc_pcm_runtime *rtd)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(comp);

	snd_pcm_set_managed_buffer_all(rtd->pcm, SNDRV_DMA_TYPE_DEV, &f->pdev->dev,
				       f->hw.buffer_bytes_max, f->hw.buffer_bytes_max);
	return 0;
}

/* One block moved: hand the FPGA the next one, and report whole periods. */
static void ea5_block_done(struct ea5_fpga *f, int pair, int dir)
{
	struct ea5_stream *s = &f->st[pair][dir];
	struct snd_pcm_runtime *rt;
	unsigned int buf, period, pos;
	bool elapsed;

	if (!s->ss || !s->running)
		return;
	rt = s->ss->runtime;
	buf = snd_pcm_lib_buffer_bytes(s->ss);
	period = snd_pcm_lib_period_bytes(s->ss);

	pos = s->pos + f->blk;
	if (pos >= buf)
		pos = 0;
	WRITE_ONCE(s->pos, pos);
	ea5_set_addr(f, pair, dir, rt->dma_addr + pos);
	elapsed = pos % period == 0;

	if (elapsed) {
		spin_unlock(&f->lock);
		snd_pcm_period_elapsed(s->ss);
		spin_lock(&f->lock);
	}
}

static irqreturn_t ea5_irq(int irq, void *data)
{
	struct ea5_fpga *f = data;
	unsigned long pending;
	int b;

	spin_lock(&f->lock);
	pending = readl(f->regs + REG_IRQ_STATUS) & 0x0f0f;
	if (!pending) {
		spin_unlock(&f->lock);
		return IRQ_NONE;
	}
	if (!f->irq_auto_clear)
		writel(pending, f->regs + REG_IRQ_STATUS);
	for_each_set_bit(b, &pending, 12)
		ea5_block_done(f, b & 7, b >= 8 ? SNDRV_PCM_STREAM_PLAYBACK :
						  SNDRV_PCM_STREAM_CAPTURE);
	spin_unlock(&f->lock);
	return IRQ_HANDLED;
}

/* ---- matrix ---------------------------------------------------------------- */

static int ea5_matrix_info(struct snd_kcontrol *kc, struct snd_ctl_elem_info *ui)
{
	ui->type = SNDRV_CTL_ELEM_TYPE_INTEGER;
	ui->count = MATRIX_SIZE;
	ui->value.integer.min = 0;
	ui->value.integer.max = MATRIX_MAX;
	return 0;
}

static int ea5_matrix_get(struct snd_kcontrol *kc, struct snd_ctl_elem_value *uv)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(snd_kcontrol_chip(kc));
	unsigned int out = kc->private_value, in;

	for (in = 0; in < MATRIX_SIZE; in++)
		uv->value.integer.value[in] = readw(f->matrix + MATRIX_REG(out, in));
	return 0;
}

static int ea5_matrix_put(struct snd_kcontrol *kc, struct snd_ctl_elem_value *uv)
{
	struct ea5_fpga *f = snd_soc_component_get_drvdata(snd_kcontrol_chip(kc));
	unsigned int out = kc->private_value, in;
	int changed = 0;

	for (in = 0; in < MATRIX_SIZE; in++) {
		long v = uv->value.integer.value[in];
		u16 old = readw(f->matrix + MATRIX_REG(out, in));

		if (v < 0 || v > MATRIX_MAX)
			return -EINVAL;
		if (old != v) {
			writew(v, f->matrix + MATRIX_REG(out, in));
			changed = 1;
		}
	}
	return changed;
}

static const DECLARE_TLV_DB_SCALE(ea5_matrix_tlv, -7830, 10, 1);

#define EA5_MATRIX(n) {							\
	.iface = SNDRV_CTL_ELEM_IFACE_MIXER,				\
	.name = "Matrix " #n " Volume",					\
	.access = SNDRV_CTL_ELEM_ACCESS_READWRITE |			\
		  SNDRV_CTL_ELEM_ACCESS_TLV_READ,			\
	.info = ea5_matrix_info,					\
	.get = ea5_matrix_get,						\
	.put = ea5_matrix_put,						\
	.tlv.p = ea5_matrix_tlv,					\
	.private_value = n,						\
}

/*
 * One control per matrix output, numbered 0-31; its 32 values are the gains
 * from each input. Port numbers: site/src/content/docs/ea/ea5.md.
 */
static const struct snd_kcontrol_new ea5_matrix_controls[] = {
	EA5_MATRIX(0),  EA5_MATRIX(1),  EA5_MATRIX(2),  EA5_MATRIX(3),
	EA5_MATRIX(4),  EA5_MATRIX(5),  EA5_MATRIX(6),  EA5_MATRIX(7),
	EA5_MATRIX(8),  EA5_MATRIX(9),  EA5_MATRIX(10), EA5_MATRIX(11),
	EA5_MATRIX(12), EA5_MATRIX(13), EA5_MATRIX(14), EA5_MATRIX(15),
	EA5_MATRIX(16), EA5_MATRIX(17), EA5_MATRIX(18), EA5_MATRIX(19),
	EA5_MATRIX(20), EA5_MATRIX(21), EA5_MATRIX(22), EA5_MATRIX(23),
	EA5_MATRIX(24), EA5_MATRIX(25), EA5_MATRIX(26), EA5_MATRIX(27),
	EA5_MATRIX(28), EA5_MATRIX(29), EA5_MATRIX(30), EA5_MATRIX(31),
};

static void ea5_matrix_init(struct ea5_fpga *f)
{
	unsigned int i;

	for (i = 0; i < MATRIX_SIZE * MATRIX_SIZE; i++)
		writew(0, f->matrix + 2 * i);
	for (i = 0; i < ARRAY_SIZE(ea5_default_routes); i++) {
		u8 out = ea5_default_routes[i][0], in = ea5_default_routes[i][1];

		writew(MATRIX_UNITY, f->matrix + MATRIX_REG(out, in));
		writew(MATRIX_UNITY, f->matrix + MATRIX_REG(out + 1, in + 1));
	}
}

/* ---- component -------------------------------------------------------------- */

static const struct snd_soc_component_driver ea5_component = {
	.name		= DRV_NAME,
	.open		= ea5_pcm_open,
	.close		= ea5_pcm_close,
	.hw_params	= ea5_pcm_hw_params,
	.prepare	= ea5_pcm_prepare,
	.trigger	= ea5_pcm_trigger,
	.pointer	= ea5_pcm_pointer,
	.pcm_new	= ea5_pcm_new,
	.controls	= ea5_matrix_controls,
	.num_controls	= ARRAY_SIZE(ea5_matrix_controls),
};

#define EA5_RATES	SNDRV_PCM_RATE_48000
#define EA5_FORMATS	(SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S24_3LE | \
			 SNDRV_PCM_FMTBIT_S32_LE)

#define EA5_DAI(n) {							\
	.name = "ea5-fpga-pcm" #n,					\
	.id = n,							\
	.playback = {							\
		.stream_name = "PCM" #n " Playback",			\
		.channels_min = 2, .channels_max = 2,			\
		.rates = EA5_RATES, .formats = EA5_FORMATS,		\
	},								\
	.capture = {							\
		.stream_name = "PCM" #n " Capture",			\
		.channels_min = 2, .channels_max = 2,			\
		.rates = EA5_RATES, .formats = EA5_FORMATS,		\
	},								\
}

static struct snd_soc_dai_driver ea5_dais[NR_PAIRS] = {
	EA5_DAI(0), EA5_DAI(1), EA5_DAI(2), EA5_DAI(3),
};

/* ---- PCI --------------------------------------------------------------------- */

static int ea5_fpga_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct device *dev = &pdev->dev;
	unsigned int buf_len, periods;
	struct ea5_fpga *f;
	int ret;

	f = devm_kzalloc(dev, sizeof(*f), GFP_KERNEL);
	if (!f)
		return -ENOMEM;
	f->pdev = pdev;
	spin_lock_init(&f->lock);

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	ret = dma_set_mask_and_coherent(dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;

	f->regs = pcim_iomap_region(pdev, 0, DRV_NAME);
	if (IS_ERR(f->regs))
		return PTR_ERR(f->regs);
	f->matrix = pcim_iomap_region(pdev, 2, DRV_NAME);
	if (IS_ERR(f->matrix))
		return PTR_ERR(f->matrix);

	f->revision = readl(f->regs + REG_REVISION);
	if (f->revision == 0xffffffff || f->revision == 0xffff)
		return dev_err_probe(dev, -ENODEV, "FPGA reads revision %#x: not configured\n",
				     f->revision);

	/*
	 * BAR1 is the FPGA's own buffer for its four pairs; a block is a quarter
	 * (rev > 4) or half of one pair's share, trimmed to a multiple of 24 bytes
	 * so 16, 24 and 32-bit stereo frames all fit whole.
	 */
	buf_len = pci_resource_len(pdev, 1) / NR_PAIRS;
	f->irq_auto_clear = f->revision > 4;
	periods = f->irq_auto_clear ? 4 : 2;
	f->blk = rounddown(buf_len / periods, 24);
	if (!f->blk)
		return dev_err_probe(dev, -ENODEV, "BAR1 too small (%u bytes per pair)\n", buf_len);
	writel(f->blk / 4, f->regs + REG_WDMA_TLP_SIZE);
	writel(f->blk / 4, f->regs + REG_RDMA_TLP_SIZE);

	writel(0, f->regs + REG_IRQ_ENABLE);
	writel(0x0f0f, f->regs + REG_IRQ_STATUS);
	writel((BUF_SS_24 << 24) | (BUF_SS_24 << 16) | (BUF_SS_24 << 8) | BUF_SS_24,
	       f->regs + REG_BUFFER_CTRL);
	ea5_update(f, REG_CONTROL, CONTROL_DAC_MUTE, CONTROL_DAC_MUTE);
	ea5_matrix_init(f);

	f->hw = (struct snd_pcm_hardware) {
		.info = SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_BLOCK_TRANSFER |
			SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID,
		.formats = EA5_FORMATS,
		.rates = EA5_RATES,
		.rate_min = 48000,
		.rate_max = 48000,
		.channels_min = 2,
		.channels_max = 2,
		.buffer_bytes_max = rounddown(BUF_BYTES_MAX, f->blk),
		.period_bytes_min = f->blk,
		.period_bytes_max = rounddown(BUF_BYTES_MAX / 2, f->blk),
		.periods_min = 2,
		.periods_max = BUF_BYTES_MAX / f->blk,
	};

	/* Managed: pcim releases the vectors after the devm IRQ is freed. */
	ret = pci_alloc_irq_vectors(pdev, 1, 1, PCI_IRQ_ALL_TYPES);
	if (ret < 0)
		return ret;
	ret = devm_request_irq(dev, pci_irq_vector(pdev, 0), ea5_irq, IRQF_SHARED,
			       DRV_NAME, f);
	if (ret)
		return ret;
	pci_set_master(pdev);
	pci_set_drvdata(pdev, f);

	ret = devm_snd_soc_register_component(dev, &ea5_component, ea5_dais,
					      ARRAY_SIZE(ea5_dais));
	if (ret)
		return ret;

	f->regset.regs = ea5_regs;
	f->regset.nregs = ARRAY_SIZE(ea5_regs);
	f->regset.base = f->regs;
	debugfs_create_regset32(DRV_NAME, 0444, NULL, &f->regset);

	dev_info(dev, "audio FPGA rev %u, %u-byte blocks, irq %d\n",
		 f->revision, f->blk, pci_irq_vector(pdev, 0));
	return 0;
}

static void ea5_fpga_quiesce(struct ea5_fpga *f)
{
	ea5_update(f, REG_CONTROL, CONTROL_DAC_MUTE, CONTROL_DAC_MUTE);
	writel(0, f->regs + REG_IRQ_ENABLE);
	writel(0, f->regs + REG_BUFFER_CTRL);
}

static void ea5_fpga_remove(struct pci_dev *pdev)
{
	struct ea5_fpga *f = pci_get_drvdata(pdev);

	debugfs_lookup_and_remove(DRV_NAME, NULL);
	ea5_fpga_quiesce(f);
}

/* Muted and stopped across reboot and kexec, as stock does at shutdown. */
static void ea5_fpga_shutdown(struct pci_dev *pdev)
{
	struct ea5_fpga *f = pci_get_drvdata(pdev);

	if (f)
		ea5_fpga_quiesce(f);
}

static const struct pci_device_id ea5_fpga_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_XILINX, EA5_FPGA_DEVICE) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ea5_fpga_ids);

static struct pci_driver ea5_fpga_pci_driver = {
	.name		= DRV_NAME,
	.id_table	= ea5_fpga_ids,
	.probe		= ea5_fpga_probe,
	.remove		= ea5_fpga_remove,
	.shutdown	= ea5_fpga_shutdown,
};

/* ---- card -------------------------------------------------------------------- */

static void ea5_put_nodes(void *data)
{
	struct snd_soc_card *card = data;
	int i;

	for (i = 0; i < card->num_aux_devs; i++)
		of_node_put(card->aux_dev[i].dlc.of_node);
}

static int ea5_card_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct device_node *np = dev->of_node;
	struct snd_soc_dai_link_component *cpus, *plat;
	struct snd_soc_dai_link *links;
	struct snd_soc_card *card;
	struct pci_dev *fpga;
	const char *fpga_name;
	int i, n, ret;

	fpga = pci_get_device(PCI_VENDOR_ID_XILINX, EA5_FPGA_DEVICE, NULL);
	if (!fpga)
		return dev_err_probe(dev, -ENODEV, "no audio FPGA on PCI (10ee:%04x)\n",
				     EA5_FPGA_DEVICE);
	fpga_name = devm_kstrdup(dev, pci_name(fpga), GFP_KERNEL);
	pci_dev_put(fpga);
	if (!fpga_name)
		return -ENOMEM;

	card = devm_kzalloc(dev, sizeof(*card), GFP_KERNEL);
	links = devm_kcalloc(dev, NR_PAIRS, sizeof(*links), GFP_KERNEL);
	cpus = devm_kcalloc(dev, NR_PAIRS, sizeof(*cpus), GFP_KERNEL);
	plat = devm_kzalloc(dev, sizeof(*plat), GFP_KERNEL);
	if (!card || !links || !cpus || !plat)
		return -ENOMEM;

	plat->name = fpga_name;
	for (i = 0; i < NR_PAIRS; i++) {
		cpus[i].name = fpga_name;
		cpus[i].dai_name = ea5_dais[i].name;
		/* The PCM's user-facing name, from the DT. */
		if (of_property_read_string_index(np, "control4,link-names", i,
						  &links[i].name))
			links[i].name = devm_kasprintf(dev, GFP_KERNEL, "PCM %d", i);
		if (!links[i].name)
			return -ENOMEM;
		links[i].stream_name = links[i].name;
		links[i].cpus = &cpus[i];
		links[i].num_cpus = 1;
		links[i].codecs = &snd_soc_dummy_dlc;
		links[i].num_codecs = 1;
		links[i].platforms = plat;
		links[i].num_platforms = 1;
	}

	card->dev = dev;
	card->owner = THIS_MODULE;
	card->dai_link = links;
	card->num_links = NR_PAIRS;
	ret = of_property_read_string(np, "model", &card->name);
	if (ret)
		card->name = "openhc-ea5";

	n = of_count_phandle_with_args(np, "control4,dsps", NULL);
	if (n > 0) {
		card->aux_dev = devm_kcalloc(dev, n, sizeof(*card->aux_dev), GFP_KERNEL);
		if (!card->aux_dev)
			return -ENOMEM;
		for (i = 0; i < n; i++)
			card->aux_dev[i].dlc.of_node = of_parse_phandle(np, "control4,dsps", i);
		card->num_aux_devs = n;
		ret = devm_add_action_or_reset(dev, ea5_put_nodes, card);
		if (ret)
			return ret;
	}

	return devm_snd_soc_register_card(dev, card);
}

static const struct of_device_id ea5_card_of_match[] = {
	{ .compatible = "control4,ea5-audio" },
	{ }
};
MODULE_DEVICE_TABLE(of, ea5_card_of_match);

static struct platform_driver ea5_card_driver = {
	.driver = {
		.name = DRV_NAME "-card",
		.of_match_table = ea5_card_of_match,
		.pm = &snd_soc_pm_ops,
	},
	.probe = ea5_card_probe,
};

static int __init ea5_fpga_init(void)
{
	int ret;

	ret = pci_register_driver(&ea5_fpga_pci_driver);
	if (ret)
		return ret;
	ret = platform_driver_register(&ea5_card_driver);
	if (ret)
		pci_unregister_driver(&ea5_fpga_pci_driver);
	return ret;
}

static void __exit ea5_fpga_exit(void)
{
	platform_driver_unregister(&ea5_card_driver);
	pci_unregister_driver(&ea5_fpga_pci_driver);
}

module_init(ea5_fpga_init);
module_exit(ea5_fpga_exit);

MODULE_DESCRIPTION("Control4 EA-5 audio FPGA");
MODULE_AUTHOR("Joseph Telaak");
MODULE_LICENSE("GPL");
