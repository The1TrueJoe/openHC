// SPDX-License-Identifier: GPL-2.0
/*
 * ALSA playback driver for the Intel CE5300's HDMI transmitter audio.
 *
 * Binds PCI 8086:2e63 ("CE Media Processor HDMI Tx Interface"). Stock never
 * plays HDMI audio through the SoC's I2S block (8086:2e60, ce5300-i2s): the
 * HDMI transmitter has an audio DMA engine of its own that reads PCM straight
 * from memory into its audio FIFO. Stock drives it from Intel's closed
 * pd_hdmi.ko; the register map here was read out of that module's HAL (its
 * function names survive: hdmi_dma_config, hdmi_audio_set_clock_recovery_n,
 * hdmi_infoframe_set_audio, ...) and every value was then checked on an EA-1
 * under openHC, where this exact sequence played a test tone through an HDMI
 * monitor's speakers: the DMA walked the ring, the FIFO never ran dry, and the
 * transmitter measured CTS = 27000 at 480p (27 MHz, N = 6144 -- exactly
 * 48 kHz).
 *
 * WHAT THE HARDWARE NEEDS
 * -----------------------
 *  - The audio master clock. The HDMI audio block divides MCLK down to
 *    128 x fs. MCLK is the SoC's audio PLL (IOSF sideband port 0x8b), which
 *    CEFDK programs at boot to stock's "mode 17", 24.576 MHz; /4 is 48 kHz.
 *    The probe reads the PLL back and refuses to guess if it is anything else.
 *  - HDMI mode. CEFDK leaves the link in DVI mode, and DVI carries no audio.
 *    The first playback switches it (+0x3000 bit 0), and first does what stock
 *    does for an HDMI sink and CEFDK never did:
 *      - the line timing at +0x3100 (active width, horizontal blanking), where
 *        the transmitter puts data islands. CEFDK leaves it 0; switched to HDMI
 *        mode that way, a monitor drops the signal outright (measured). It is
 *        read off the VDC's pipe A, which CEFDK set to the mode it drives;
 *      - the GCP position at +0x3104 (front porches: where VSYNC starts), which
 *        the GCP has to go out against. CEFDK leaves it 0, and then a GCP
 *        lands wherever and a sink takes it some of the time (heard: a tone
 *        that keeps cutting out). Also from pipe A;
 *      - an AVI infoframe that keeps the picture as it was (RGB, full range);
 *      - a GCP clearing AV mute (bit 15; bit 16 sets it -- swapped, it is the
 *        same cutting out, and silence once the GCP position is right).
 *  - Channel status and an audio infoframe, both 2-channel LPCM at 48 kHz.
 *
 * Periods are paced by a timer from the DMA position, as in ce5300-i2s: no
 * interrupt is needed and none is routed reliably on the kernels openHC boots.
 *
 * Once up, the link never stops sending audio: between streams the engine
 * loops a short silent buffer, and a stream starts by linking its ring in
 * after it and ends by linking it back out. Stopping the audio unit instead
 * cost the start of every following stream, while the sink locked on again
 * (heard: each speaker-test channel and each clip after the first lost its
 * first syllable).
 *
 * Periods are whole and 64-byte aligned: the engine walks its source in
 * 32-byte steps, and a ring that is not exactly the ALSA buffer drifts against
 * it (heard: OpenAL's 4458-frame buffer of 1115-frame periods played choppy
 * and fast).
 *
 * Samples are MSB-aligned in their 32-bit FIFO word (S32_LE; bits 7:0 are not
 * sent). Heard: the same tone LSB-aligned is ~60 dB down, inaudible.
 */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/hrtimer.h>
#include <linux/io.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/unaligned.h>
#include <sound/core.h>
#include <sound/initval.h>
#include <sound/pcm.h>

#define DRV_NAME		"ce5300-hdmi"

/* ---- Audio DMA: one context at BAR0 +0x00, ce5300-i2s.h's layout ------- */
#define HDMI_DMA_NEXT_DESC	0x0c	/* start: the head descriptor goes here */
#define HDMI_DMA_CUR_SRC	0x10	/* the playback position */
#define HDMI_DMA_DST		0x14
#define HDMI_DMA_SIZE		0x18	/* bytes left in the current descriptor */
#define HDMI_DMA_CTRL		0x1c
#define HDMI_DMA_STOP		0x20	/* bit 0: stop the context */

/*
 * The mode word pd_hdmi starts the engine with (hdmi_dma_get_default_flags,
 * 0x6096, with bit 30 as in every Intel CE audio DMA), and the destination it
 * writes, a placeholder: the FIFO is the engine's own, not an address.
 */
#define HDMI_DMA_FLAGS		0x40006096
#define HDMI_DMA_DST_FIFO	0x10101010
#define HDMI_DMA_RUN		BIT(31)	/* reads set while the context runs */

/* ---- General ----------------------------------------------------------- */
#define HDMI_CLOCKS		0x1000
#define   HDMI_CLK_AUDIO	BIT(2)	/* hdmi_general_audio_clock_enable */
#define HDMI_OPMODE		0x3000
#define   HDMI_OPMODE_HDMI	BIT(0)	/* clear = DVI */
#define HDMI_LINE_TIMING	0x3100	/* hdmi_video_set_tv_timings: active << 16 | hblank */
#define HDMI_GCP_LINE		0x3104	/* hdmi_hal_gcp_line_set: [29:16] h, [13:0] v front porch */
#define   HDMI_GCP_LINE_ENABLE	BIT(30)
#define   HDMI_GCP_LINE_KEEP	BIT(31)	/* stock preserves it */

/* ---- Audio ------------------------------------------------------------- */
#define HDMI_AUD_CONFIG		0x4000
#define   HDMI_AUD_ENABLE	BIT(0)
/* [1] layout, [3:2] format, [5:4] channels, [6] fs > 96 kHz, [7] B, [8] flat,
 * [9] not-valid: all zero is two-channel LPCM, valid samples. */
#define HDMI_AUD_CTS_HW		0x4004	/* CTS as the transmitter measures it */
#define HDMI_AUD_N		0x4008
#define HDMI_AUD_CTS		0x400c	/* [19:0] CTS, [20] use it (bypass); 0 = measure */
#define HDMI_AUD_DIVIDER	0x4010	/* MCLK / divider = 128 x fs */
#define HDMI_AUD_FIFO_THRESH	0x4014
#define HDMI_AUD_FIFO_LEVEL	0x4018
#define HDMI_AUD_CHSTAT0	0x4050	/* IEC 60958 channel status bytes 0-3 */
#define HDMI_AUD_CHSTAT1	0x4054	/* ... byte 4 */
#define HDMI_AUD_CHSTAT_SEND	0x4058

/* Infoframe slot n: header at 0x4100 + 0x20n, payload words after it, control
 * at 0x4180 + 4n ([0] enable, [1] send once rather than every frame). */
#define HDMI_IF_HDR(n)		(0x4100 + (n) * 0x20)
#define HDMI_IF_PB(n, w)	(HDMI_IF_HDR(n) + 4 + (w) * 4)
#define HDMI_IF_CTRL(n)		(0x4180 + (n) * 4)
#define   HDMI_IF_ENABLE	BIT(0)
#define   HDMI_IF_ONCE		BIT(1)
#define HDMI_IF_SLOT_AVI	0
#define HDMI_IF_SLOT_AUDIO	1
#define HDMI_GCP		0x428c	/* hdmi_hal_gcp_avmute_set */
#define   HDMI_GCP_CLEAR_AVMUTE	BIT(15)
#define   HDMI_GCP_SET_AVMUTE	BIT(16)
#define   HDMI_GCP_SEND		BIT(5)

/* The display controller (VDC, 8086:2e61): pipe A's timing, i9xx layout --
 * (total - 1) << 16 | (active - 1), and blank/sync (end - 1) << 16 | (start - 1). */
#define VDC_PCI_DEVICE		0x2e61
#define VDC_TIMING_A		0x60000
#define VDC_HTOTAL		0x00
#define VDC_HBLANK		0x04
#define VDC_HSYNC		0x08
#define VDC_VBLANK		0x10
#define VDC_VSYNC		0x14
#define VDC_TIMING_LEN		0x18

/* 48 kHz from a 24.576 MHz MCLK; N from pd_hdmi's table (6144 at every pixel
 * clock CEFDK sets except the /1.001 ones, and the CTS is measured anyway). */
#define HDMI_RATE		48000
#define HDMI_DIVIDER_48K	4
#define HDMI_N_48K		6144
#define HDMI_FIFO_THRESHOLD	0x40

/* ---- The audio PLL, over the IOSF sideband (host bridge 00:00.0) ------- */
#define IOSF_MCR		0xd0
#define IOSF_MDR		0xd4
#define IOSF_MCRX		0xd8
#define IOSF_OP_CR_READ		0x06
#define IOSF_PORT_APLL		0x8b
/* apll_cr[1] and [2] of stock's AUDIO_HAL_PLL_MODE17 (24.576 MHz, internal) */
#define APLL_MODE17_CR1		0x00090020
#define APLL_MODE17_CR2		0x00063309

/*
 * The descriptor the engine walks. pd_hdmi's ring has a 24-byte stride
 * (create_dma_descriptors_chain): the five hardware words plus a tag.
 */
struct hdmi_dma_desc {
	__le32 next_desc;
	__le32 src_size;
	__le32 src_start;
	__le32 dst_start;
	__le32 flags_mode;
	__le32 tag;
} __packed;

#define HDMI_PERIODS_MAX	32
#define HDMI_BUFFER_BYTES_MAX	(256 * 1024)
#define HDMI_BYTES_ALIGN	64
/* What the engine loops between streams: 128 frames, 2.7 ms. */
#define HDMI_SILENCE_BYTES	1024

static int index = SNDRV_DEFAULT_IDX1;
module_param(index, int, 0444);
MODULE_PARM_DESC(index, "Index value for the CE5300 HDMI soundcard.");

struct ce5300_hdmi {
	struct pci_dev		*pdev;
	void __iomem		*base;
	struct snd_card		*card;
	struct snd_pcm_substream *substream;

	struct hdmi_dma_desc	*ring;		/* HDMI_PERIODS_MAX, then the idle one */
	dma_addr_t		ring_phys;
	struct hdmi_dma_desc	*idle;		/* loops the silence between streams */
	dma_addr_t		idle_phys;
	dma_addr_t		silence_phys;
	bool			streaming;	/* audio unit on, engine walking */

	struct hrtimer		tick;		/* one period; reports elapsed periods */
	ktime_t			period;
	atomic_t		running;
};

static u32 hdmi_read(struct ce5300_hdmi *h, u32 off)
{
	return readl(h->base + off);
}

static void hdmi_write(struct ce5300_hdmi *h, u32 off, u32 val)
{
	writel(val, h->base + off);
}

static void hdmi_update(struct ce5300_hdmi *h, u32 off, u32 mask, u32 val)
{
	hdmi_write(h, off, (hdmi_read(h, off) & ~mask) | (val & mask));
}

static u32 iosf_cr_read(struct pci_dev *host, u8 port, u32 off)
{
	u32 val;

	pci_write_config_dword(host, IOSF_MCRX, off & ~0xff);
	pci_write_config_dword(host, IOSF_MCR, (IOSF_OP_CR_READ << 24) |
			       (port << 16) | ((off & 0xff) << 8) | 0xf0);
	pci_read_config_dword(host, IOSF_MDR, &val);
	return val;
}

/* Is MCLK the 24.576 MHz the divider assumes? Read-only; CEFDK set it. */
static int ce5300_hdmi_check_pll(struct ce5300_hdmi *h)
{
	struct pci_dev *host = pci_get_domain_bus_and_slot(0, 0, PCI_DEVFN(0, 0));
	u32 cr1, cr2;

	if (!host)
		return -ENODEV;
	cr1 = iosf_cr_read(host, IOSF_PORT_APLL, 1);
	cr2 = iosf_cr_read(host, IOSF_PORT_APLL, 2);
	pci_dev_put(host);

	if (cr1 != APLL_MODE17_CR1 || cr2 != APLL_MODE17_CR2) {
		dev_err(&h->pdev->dev,
			"audio PLL not at 24.576 MHz (cr1 %#x cr2 %#x); refusing to guess a divider\n",
			cr1, cr2);
		return -ENODEV;
	}
	return 0;
}

/* Write a 3-byte header + checksummed payload into an infoframe slot. */
static void hdmi_infoframe(struct ce5300_hdmi *h, unsigned int slot,
			   u8 type, u8 version, const u8 *pb, unsigned int len)
{
	u8 buf[28] = { };
	u8 sum = type + version + len;
	unsigned int i;

	for (i = 0; i < len; i++)
		sum += pb[i];
	buf[0] = -sum;
	memcpy(&buf[1], pb, len);

	hdmi_update(h, HDMI_IF_CTRL(slot), HDMI_IF_ENABLE, 0);
	hdmi_write(h, HDMI_IF_HDR(slot), type | version << 8 | len << 16);
	for (i = 0; i < 7; i++)
		hdmi_write(h, HDMI_IF_PB(slot, i), get_unaligned_le32(&buf[i * 4]));
	hdmi_update(h, HDMI_IF_CTRL(slot), HDMI_IF_ENABLE | HDMI_IF_ONCE,
		    HDMI_IF_ENABLE);
}

struct hdmi_link_timing {
	u32 line;	/* HDMI_LINE_TIMING */
	u32 gcp;	/* HDMI_GCP_LINE, without the enable */
};

/* Pipe A's timing as CEFDK set the VDC up, in the transmitter's two words. */
static int ce5300_hdmi_link_timing(struct ce5300_hdmi *h, struct hdmi_link_timing *t)
{
	struct pci_dev *vdc = pci_get_device(PCI_VENDOR_ID_INTEL, VDC_PCI_DEVICE, NULL);
	void __iomem *regs;
	u32 htotal, hblank, hsync, vblank, vsync, active, blank, hfp, vfp;

	if (!vdc)
		return -ENODEV;
	/* Read-only, and not ours to claim: map the registers, no request. */
	regs = ioremap(pci_resource_start(vdc, 0) + VDC_TIMING_A, VDC_TIMING_LEN);
	pci_dev_put(vdc);
	if (!regs)
		return -ENOMEM;
	htotal = readl(regs + VDC_HTOTAL);
	hblank = readl(regs + VDC_HBLANK);
	hsync = readl(regs + VDC_HSYNC);
	vblank = readl(regs + VDC_VBLANK);
	vsync = readl(regs + VDC_VSYNC);
	iounmap(regs);

	active = (htotal & 0xfff) + 1;
	blank = ((hblank >> 16) & 0xfff) - (hblank & 0xfff);
	/* hdmi_hal_gcp_line_set's arguments: sync start - blank start */
	hfp = (hsync & 0xfff) - (hblank & 0xfff);
	vfp = (vsync & 0xfff) - (vblank & 0xfff);
	if (!active || active > 0xfff || !blank || blank > 0x3ff ||
	    !hfp || hfp > blank || !vfp || vfp > 0x3fff) {
		dev_err(&h->pdev->dev,
			"VDC timing unreadable (h %#x/%#x/%#x v %#x/%#x)\n",
			htotal, hblank, hsync, vblank, vsync);
		return -EINVAL;
	}
	t->line = active << 16 | blank;
	t->gcp = hfp << 16 | vfp;
	return 0;
}

/*
 * DVI -> HDMI. Done on first playback, not at probe, so a box that never plays
 * sound keeps the link exactly as CEFDK left it. The AVI infoframe says RGB,
 * full range -- what a sink assumes of a DVI source -- so the picture does not
 * change when audio starts.
 */
static int ce5300_hdmi_link_to_hdmi(struct ce5300_hdmi *h)
{
	static const u8 avi[13] = { 0x00, 0x08, 0x08 };	/* RGB; aspect as coded; Q full */
	struct hdmi_link_timing t;
	int ret;

	if (hdmi_read(h, HDMI_OPMODE) & HDMI_OPMODE_HDMI)
		return 0;
	ret = ce5300_hdmi_link_timing(h, &t);
	if (ret)
		return ret;
	hdmi_write(h, HDMI_LINE_TIMING, t.line);
	hdmi_update(h, HDMI_GCP_LINE, ~HDMI_GCP_LINE_KEEP, HDMI_GCP_LINE_ENABLE | t.gcp);
	hdmi_infoframe(h, HDMI_IF_SLOT_AVI, 0x82, 2, avi, sizeof(avi));
	hdmi_update(h, HDMI_GCP, HDMI_GCP_SET_AVMUTE | HDMI_GCP_CLEAR_AVMUTE | HDMI_GCP_SEND,
		    HDMI_GCP_CLEAR_AVMUTE | HDMI_GCP_SEND);
	hdmi_update(h, HDMI_OPMODE, HDMI_OPMODE_HDMI, HDMI_OPMODE_HDMI);
	dev_info(&h->pdev->dev,
		 "link switched from DVI to HDMI mode for audio (line %u + %u, porches %u/%u)\n",
		 t.line >> 16, t.line & 0xffff, t.gcp >> 16, t.gcp & 0xffff);
	return 0;
}

/* ---- PCM ---------------------------------------------------------------- */

static const struct snd_pcm_hardware ce5300_hdmi_pcm_hw = {
	.info			= SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID |
				  SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_BLOCK_TRANSFER,
	.formats		= SNDRV_PCM_FMTBIT_S32_LE,
	.rates			= SNDRV_PCM_RATE_48000,
	.rate_min		= HDMI_RATE,
	.rate_max		= HDMI_RATE,
	.channels_min		= 2,
	.channels_max		= 2,
	.buffer_bytes_max	= HDMI_BUFFER_BYTES_MAX,
	.period_bytes_min	= 1024,
	.period_bytes_max	= HDMI_BUFFER_BYTES_MAX / 2,
	.periods_min		= 2,
	.periods_max		= HDMI_PERIODS_MAX,
};

static int ce5300_hdmi_pcm_open(struct snd_pcm_substream *ss)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);

	struct snd_pcm_runtime *rt = ss->runtime;
	int ret;

	rt->hw = ce5300_hdmi_pcm_hw;
	ret = snd_pcm_hw_constraint_integer(rt, SNDRV_PCM_HW_PARAM_PERIODS);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_step(rt, 0, SNDRV_PCM_HW_PARAM_PERIOD_BYTES,
					 HDMI_BYTES_ALIGN);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_step(rt, 0, SNDRV_PCM_HW_PARAM_BUFFER_BYTES,
					 HDMI_BYTES_ALIGN);
	if (ret < 0)
		return ret;
	h->substream = ss;
	return 0;
}

static int ce5300_hdmi_pcm_close(struct snd_pcm_substream *ss)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);

	h->substream = NULL;
	return 0;
}

static void hdmi_desc_set(struct hdmi_dma_desc *d, dma_addr_t next,
			  dma_addr_t src, unsigned int bytes)
{
	d->next_desc  = cpu_to_le32(next);
	d->src_size   = cpu_to_le32(bytes);
	d->src_start  = cpu_to_le32(src);
	d->dst_start  = cpu_to_le32(HDMI_DMA_DST_FIFO);
	d->flags_mode = cpu_to_le32(HDMI_DMA_FLAGS);
	d->tag        = 0;
}

/*
 * First playback: HDMI mode, the audio unit, and the engine on the idle
 * descriptor, which loops the silence until a stream links its ring in.
 */
static int ce5300_hdmi_link_up(struct ce5300_hdmi *h)
{
	static const u8 audio_if[10] = { 0x01 };	/* 2 channels, rest from the stream */
	int ret;

	if (h->streaming)
		return 0;
	ret = ce5300_hdmi_link_to_hdmi(h);
	if (ret)
		return ret;
	hdmi_update(h, HDMI_CLOCKS, HDMI_CLK_AUDIO, HDMI_CLK_AUDIO);

	hdmi_write(h, HDMI_AUD_CONFIG, 0);
	hdmi_write(h, HDMI_AUD_DIVIDER, HDMI_DIVIDER_48K);
	hdmi_write(h, HDMI_AUD_FIFO_THRESH, HDMI_FIFO_THRESHOLD);
	hdmi_write(h, HDMI_AUD_N, HDMI_N_48K);
	hdmi_write(h, HDMI_AUD_CTS, 0);

	/* Consumer, LPCM, copying permitted; fs 48 kHz; 24-bit words. */
	hdmi_write(h, HDMI_AUD_CHSTAT0, 0x02000004);
	hdmi_write(h, HDMI_AUD_CHSTAT1, 0x0000000b);
	hdmi_write(h, HDMI_AUD_CHSTAT_SEND, 1);

	hdmi_infoframe(h, HDMI_IF_SLOT_AUDIO, 0x84, 1, audio_if, sizeof(audio_if));

	hdmi_desc_set(h->idle, h->idle_phys, h->silence_phys, HDMI_SILENCE_BYTES);
	wmb();
	/* pd_hdmi's hdmi_dma_custom_start, then hdmi_audio_enable. */
	hdmi_write(h, HDMI_DMA_NEXT_DESC, h->idle_phys);
	hdmi_write(h, HDMI_DMA_SIZE, 0);
	hdmi_write(h, HDMI_DMA_DST, HDMI_DMA_DST_FIFO);
	hdmi_write(h, HDMI_DMA_CTRL, HDMI_DMA_FLAGS);
	hdmi_update(h, HDMI_AUD_CONFIG, HDMI_AUD_ENABLE, HDMI_AUD_ENABLE);
	h->streaming = true;
	return 0;
}

static void ce5300_hdmi_link_down(struct ce5300_hdmi *h)
{
	hdmi_update(h, HDMI_DMA_STOP, BIT(0), BIT(0));
	hdmi_update(h, HDMI_AUD_CONFIG, HDMI_AUD_ENABLE, 0);
	h->streaming = false;
}

static bool ce5300_hdmi_idle(struct ce5300_hdmi *h)
{
	u32 src = hdmi_read(h, HDMI_DMA_CUR_SRC);

	return src - (u32)h->silence_phys <= HDMI_SILENCE_BYTES;
}

/*
 * The ring: one descriptor per period, chained in a circle with no STOP bit
 * anywhere. pd_hdmi instead marks the newest descriptor STOP (bit 28) because
 * it is fed buffer by buffer; a cyclic ALSA buffer is always full. sync_stop
 * has seen the engine back on the idle descriptor, so the ring is free.
 */
static int ce5300_hdmi_pcm_prepare(struct snd_pcm_substream *ss)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	unsigned int period_bytes = snd_pcm_lib_period_bytes(ss);
	unsigned int i;
	int ret;

	if (rt->periods > HDMI_PERIODS_MAX)
		return -EINVAL;

	ret = ce5300_hdmi_link_up(h);
	if (ret)
		return ret;

	for (i = 0; i < rt->periods; i++)
		hdmi_desc_set(&h->ring[i],
			      h->ring_phys + ((i + 1) % rt->periods) * sizeof(h->ring[i]),
			      rt->dma_addr + i * period_bytes, period_bytes);
	/* Publish every descriptor before trigger links the ring in. */
	wmb();

	h->period = ns_to_ktime(div_u64((u64)rt->period_size * NSEC_PER_SEC, rt->rate));
	return 0;
}

static int ce5300_hdmi_pcm_trigger(struct snd_pcm_substream *ss, int cmd)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	unsigned int i;

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		/*
		 * The engine latches a descriptor's next pointer as it loads it,
		 * so the ring starts after one or two more laps of the silence.
		 */
		h->idle->next_desc = cpu_to_le32(h->ring_phys);
		wmb();
		atomic_set(&h->running, 1);
		hrtimer_start(&h->tick, h->period, HRTIMER_MODE_REL_SOFT);
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
		atomic_set(&h->running, 0);
		if (!hrtimer_callback_running(&h->tick))
			hrtimer_try_to_cancel(&h->tick);
		/*
		 * Up to two more periods play before the engine is back on the
		 * silence (the latch again): make them silent too.
		 */
		memset(rt->dma_area, 0, snd_pcm_lib_buffer_bytes(ss));
		h->idle->next_desc = cpu_to_le32(h->idle_phys);
		for (i = 0; i < rt->periods; i++)
			h->ring[i].next_desc = cpu_to_le32(h->idle_phys);
		wmb();
		return 0;
	default:
		return -EINVAL;
	}
}

/*
 * Wait out a tick that was mid-flight when trigger stopped the stream, and the
 * engine's last periods of the ring: prepare rewrites it and hw_free may hand
 * the buffer back. If the engine never gets back to the silence, stop it; the
 * next prepare brings the link up again.
 */
static int ce5300_hdmi_pcm_sync_stop(struct snd_pcm_substream *ss)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	unsigned long timeout_us;
	bool idle;

	hrtimer_cancel(&h->tick);
	if (!h->streaming)
		return 0;
	timeout_us = 4 * div_u64((u64)rt->period_size * USEC_PER_SEC, rt->rate) +
		     10 * USEC_PER_MSEC;
	if (read_poll_timeout(ce5300_hdmi_idle, idle, idle, 500, timeout_us, false, h)) {
		dev_warn(&h->pdev->dev, "engine not back on the silence; stopping it\n");
		ce5300_hdmi_link_down(h);
	}
	return 0;
}

/* One period has gone by: let ALSA read the position and refill. */
static enum hrtimer_restart ce5300_hdmi_tick(struct hrtimer *t)
{
	struct ce5300_hdmi *h = container_of(t, struct ce5300_hdmi, tick);

	if (!atomic_read(&h->running))
		return HRTIMER_NORESTART;
	/* May stop the stream (xrun, drain), which clears running. */
	snd_pcm_period_elapsed(h->substream);
	if (!atomic_read(&h->running))
		return HRTIMER_NORESTART;
	hrtimer_forward_now(t, h->period);
	return HRTIMER_RESTART;
}

/* The context's current source address walks the buffer (measured). */
static snd_pcm_uframes_t ce5300_hdmi_pcm_pointer(struct snd_pcm_substream *ss)
{
	struct ce5300_hdmi *h = snd_pcm_substream_chip(ss);
	struct snd_pcm_runtime *rt = ss->runtime;
	u32 off = hdmi_read(h, HDMI_DMA_CUR_SRC) - (u32)rt->dma_addr;

	if (off >= snd_pcm_lib_buffer_bytes(ss))
		return 0;
	return bytes_to_frames(rt, off);
}

static const struct snd_pcm_ops ce5300_hdmi_pcm_ops = {
	.open		= ce5300_hdmi_pcm_open,
	.close		= ce5300_hdmi_pcm_close,
	.prepare	= ce5300_hdmi_pcm_prepare,
	.trigger	= ce5300_hdmi_pcm_trigger,
	.sync_stop	= ce5300_hdmi_pcm_sync_stop,
	.pointer	= ce5300_hdmi_pcm_pointer,
};

/* ---- PCI --------------------------------------------------------------- */

/*
 * No register access here: BAR0 is a later devres than the card, so it is
 * already unmapped when the card is freed. ce5300_hdmi_quiesce stopped the
 * engine before that.
 */
static void ce5300_hdmi_free(struct snd_card *card)
{
	struct ce5300_hdmi *h = card->private_data;

	hrtimer_cancel(&h->tick);
}

/* A devres after BAR0 and the ring: stop the engine before either goes. */
static void ce5300_hdmi_quiesce(void *data)
{
	struct ce5300_hdmi *h = data;

	if (h->streaming)
		ce5300_hdmi_link_down(h);
}

static int ce5300_hdmi_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct snd_card *card;
	struct ce5300_hdmi *h;
	struct snd_pcm *pcm;
	int ret;

	ret = snd_devm_card_new(&pdev->dev, index, "HDMI", THIS_MODULE,
				sizeof(*h), &card);
	if (ret)
		return ret;
	h = card->private_data;
	h->card = card;
	h->pdev = pdev;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	h->base = pcim_iomap(pdev, 0, 0);
	if (!h->base)
		return -ENOMEM;
	pci_set_master(pdev);

	ret = ce5300_hdmi_check_pll(h);
	if (ret)
		return ret;

	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;
	/* The ring, the idle descriptor after it, then the silence it loops. */
	h->ring = dmam_alloc_coherent(&pdev->dev,
				      (HDMI_PERIODS_MAX + 1) * sizeof(*h->ring) +
				      HDMI_BYTES_ALIGN + HDMI_SILENCE_BYTES,
				      &h->ring_phys, GFP_KERNEL);
	if (!h->ring)
		return -ENOMEM;
	h->idle = &h->ring[HDMI_PERIODS_MAX];
	h->idle_phys = h->ring_phys + HDMI_PERIODS_MAX * sizeof(*h->ring);
	h->silence_phys = ALIGN(h->idle_phys + sizeof(*h->idle), HDMI_BYTES_ALIGN);
	ret = devm_add_action_or_reset(&pdev->dev, ce5300_hdmi_quiesce, h);
	if (ret)
		return ret;

	hrtimer_setup(&h->tick, ce5300_hdmi_tick, CLOCK_MONOTONIC, HRTIMER_MODE_REL_SOFT);
	card->private_free = ce5300_hdmi_free;

	ret = snd_pcm_new(card, "HDMI", 0, 1, 0, &pcm);
	if (ret)
		return ret;
	pcm->private_data = h;
	strscpy(pcm->name, "HDMI");
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &ce5300_hdmi_pcm_ops);
	snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_DEV, &pdev->dev,
				       HDMI_BUFFER_BYTES_MAX, HDMI_BUFFER_BYTES_MAX);

	strscpy(card->driver, "CE5300HDMI");
	strscpy(card->shortname, "CE5300 HDMI");
	snprintf(card->longname, sizeof(card->longname),
		 "Intel CE5300 HDMI audio at %pR", &pdev->resource[0]);

	ret = snd_card_register(card);
	if (ret)
		return ret;

	dev_info(&pdev->dev, "HDMI audio: 2ch LPCM 48 kHz, link in %s mode\n",
		 hdmi_read(h, HDMI_OPMODE) & HDMI_OPMODE_HDMI ? "HDMI" : "DVI");
	return 0;
}

static const struct pci_device_id ce5300_hdmi_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x2e63) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce5300_hdmi_ids);

static struct pci_driver ce5300_hdmi_driver = {
	.name		= DRV_NAME,
	.id_table	= ce5300_hdmi_ids,
	.probe		= ce5300_hdmi_probe,
};
module_pci_driver(ce5300_hdmi_driver);

MODULE_DESCRIPTION("Intel CE5300 HDMI transmitter audio");
MODULE_LICENSE("GPL");
