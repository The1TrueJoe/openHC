// SPDX-License-Identifier: GPL-2.0
/*
 * ASoC platform + CPU-DAI SKELETON for Control4 EA1 HDMI audio.
 *
 *     CE5300 HDMI-audio FIFO/DMA  --I2S/SPDIF internal-->  integrated HDMI TX
 *
 * EA1 has no ADAU1451 and no I2C codec (that is EA3's audio-dsp path). Its only
 * audio output is HDMI audio driven by the SoC's integrated HDMI transmitter.
 * This file is the HDMI-side counterpart to ce5300-i2s.c, but it is a SKELETON:
 * it registers a valid, no-op ASoC platform component + CPU DAI so a card can be
 * built and `aplay -l` can list a device, and every hardware operation is a
 * clearly-marked TODO stub. IT PERFORMS NO REGISTER ACCESS.
 *
 * ============================ READ THIS FIRST ============================
 * The CE5300 HDMI-audio register map is UN-REVERSE-ENGINEERED. The render path
 * lives in Intel's proprietary SMD stack (`ismdaudio` + the GDL `pd_hdmi` /
 * `gdl_server` userspace) which is NOT in Control4's GPL drop, and the whole
 * audio block sits behind the Clock-and-Reset Controller (PCI 00:00.2,
 * 8086:2e52) that CEFDK never ungates. Two measured facts bound what this driver
 * may do:
 *
 *   - Reading an un-ungated audio TX/DMA register HANGS THE SoC DEAD. It needs a
 *     power cycle; there is no recovery from software. ce5300-i2s.c wedged an
 *     EA3 reading its TX block at 0x2004 even with the PCI function enabled.
 *   - Poking the device with devmem at 0xc000/0xf000 hung an EA3 just as hard.
 *
 * Therefore this skeleton is a PLATFORM driver bound to a software platform
 * device (name "ce5300-hdmi" registered by ce5300-ea1-board.c), NOT a PCI driver
 * bound to 8086:2e5f/2e60. It never maps a BAR and never touches MMIO. Filling
 * in real I/O is a SEPARATE, SUPERVISED reverse-engineering effort — see
 * HDMI-AUDIO-RE.md in this directory for the register-discovery TODO and the
 * supervised test plan. Do NOT add register reads/writes here unattended.
 * =========================================================================
 */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>

#define DRV_NAME		"ce5300-hdmi"
#define CE5300_HDMI_BUFFER_BYTES_MAX	(256 * 1024)
#define CE5300_HDMI_PERIODS_MAX		32

/*
 * HDMI audio is inherently playback-only and, on this SoC, LPCM stereo at the
 * HDMI-friendly rates. Capture and compressed-bitstream passthrough (DTS/DD/AAC,
 * which the stock /etc/hdmi_hpd.cfg advertised) are out of scope for the
 * skeleton. Kept intentionally narrow so a future backend grows into it rather
 * than has to walk anything back.
 */
static const struct snd_pcm_hardware ce5300_hdmi_pcm_hw = {
	.info			= SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID |
				  SNDRV_PCM_INFO_INTERLEAVED |
				  SNDRV_PCM_INFO_BLOCK_TRANSFER,
	.formats		= SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S24_LE |
				  SNDRV_PCM_FMTBIT_S32_LE,
	.rates			= SNDRV_PCM_RATE_32000 | SNDRV_PCM_RATE_44100 |
				  SNDRV_PCM_RATE_48000,
	.rate_min		= 32000,
	.rate_max		= 48000,
	.channels_min		= 2,
	.channels_max		= 2,
	.buffer_bytes_max	= CE5300_HDMI_BUFFER_BYTES_MAX,
	.period_bytes_min	= 1024,
	.period_bytes_max	= CE5300_HDMI_BUFFER_BYTES_MAX / 2,
	.periods_min		= 2,
	.periods_max		= CE5300_HDMI_PERIODS_MAX,
};

static int ce5300_hdmi_pcm_open(struct snd_soc_component *comp,
				struct snd_pcm_substream *ss)
{
	/*
	 * Advertise the hw constraints so userspace negotiates a sane format.
	 * This is pure software — no device touched.
	 */
	snd_soc_set_runtime_hwparams(ss, &ce5300_hdmi_pcm_hw);
	return 0;
}

static int ce5300_hdmi_pcm_close(struct snd_soc_component *comp,
				 struct snd_pcm_substream *ss)
{
	return 0;
}

/*
 * TODO (SUPERVISED RE — HDMI-AUDIO-RE.md): program the HDMI-audio FIFO/DMA.
 *
 * The real work here is, roughly:
 *   - build the HDMI-audio FIFO/DMA descriptor ring for `rt->periods` periods
 *     (the CE5300 I2S DMA engine in ce5300-i2s.h is the closest known relative;
 *     whether the HDMI path reuses that same linked-list engine or a separate
 *     one is UNKNOWN until the block is RE'd);
 *   - set the audio sample format / channel count in the HDMI TX;
 *   - compute and load the ACR N / CTS values for the negotiated rate vs the
 *     HDMI TMDS clock (the N-CTS pair the HDMI sink uses to recover the audio
 *     clock);
 *   - assemble and arm the HDMI Audio InfoFrame (channel count, coding type,
 *     sample size, speaker allocation) and the channel-status/IEC bits.
 *
 * NONE of the above is done here and NONE of it is safe to attempt until the
 * audio block has been ungated behind the CRC (8086:2e52) and the register map
 * recovered under supervision. Returning 0 keeps the PCM openable (so the card
 * enumerates and the pipeline can be exercised end-to-end as a no-op) without
 * pretending to move samples.
 */
static int ce5300_hdmi_pcm_prepare(struct snd_soc_component *comp,
				   struct snd_pcm_substream *ss)
{
	dev_dbg(comp->dev, "prepare: HDMI-audio FIFO/DMA/ACR/InfoFrame TODO (no-op skeleton)\n");
	return 0;
}

/*
 * TODO (SUPERVISED RE): START must arm the HDMI-audio FIFO/DMA and unmute the
 * HDMI audio stream; STOP must halt it. All of this is register access into the
 * un-RE'd, clock-gated audio block — see the banner at the top of the file. The
 * skeleton accepts the trigger transitions so ALSA's state machine is happy and
 * does nothing to the hardware.
 */
static int ce5300_hdmi_pcm_trigger(struct snd_soc_component *comp,
				   struct snd_pcm_substream *ss, int cmd)
{
	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
	case SNDRV_PCM_TRIGGER_RESUME:
	case SNDRV_PCM_TRIGGER_PAUSE_RELEASE:
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		/* TODO: arm/halt the HDMI-audio FIFO/DMA here (un-RE'd). */
		return 0;
	default:
		return -EINVAL;
	}
}

/*
 * UNIMPLEMENTED ON PURPOSE, same reasoning as ce5300-i2s.c: ALSA needs a real
 * byte position within the ring and the register that carries it is un-RE'd.
 * Fabricating a position makes playback look like it works while drifting, a far
 * worse failure than an honest zero. Returning 0 is safe for a no-op skeleton
 * because the stream never actually runs.
 */
static snd_pcm_uframes_t ce5300_hdmi_pcm_pointer(struct snd_soc_component *comp,
						 struct snd_pcm_substream *ss)
{
	return 0;
}

static int ce5300_hdmi_pcm_new(struct snd_soc_component *comp,
			       struct snd_soc_pcm_runtime *rtd)
{
	/*
	 * Preallocate a buffer so the PCM is openable (aplay/speaker-test can
	 * negotiate and run as a no-op). CONTINUOUS, not DEV: this component binds
	 * a software platform device with no DMA mask, so device-coherent DMA
	 * would warn; continuous pages need no DMA-capable device. A real backend
	 * will switch this to SNDRV_DMA_TYPE_DEV against the mapped audio function.
	 * Touches no audio register either way.
	 */
	snd_pcm_set_managed_buffer_all(rtd->pcm, SNDRV_DMA_TYPE_CONTINUOUS,
				       comp->dev,
				       CE5300_HDMI_BUFFER_BYTES_MAX,
				       CE5300_HDMI_BUFFER_BYTES_MAX);
	return 0;
}

static const struct snd_soc_component_driver ce5300_hdmi_component = {
	.name		= DRV_NAME,
	.open		= ce5300_hdmi_pcm_open,
	.close		= ce5300_hdmi_pcm_close,
	.prepare	= ce5300_hdmi_pcm_prepare,
	.trigger	= ce5300_hdmi_pcm_trigger,
	.pointer	= ce5300_hdmi_pcm_pointer,
	/* .pcm_new, not .pcm_construct: this kernel's snd_soc_component_driver
	 * still calls the member pcm_new (same signature). See ce5300-i2s.c. */
	.pcm_new	= ce5300_hdmi_pcm_new,
};

/*
 * The CPU DAI. Like ce5300-i2s's DAI it is a pure stub: the HDMI transmitter is
 * clocked by the SoC's own PLLs (not by anything we program per-DAI here), so
 * there is no clock direction or format to set from this side. The DAI exists so
 * the machine driver has a COMP_CPU to name.
 */
static struct snd_soc_dai_driver ce5300_hdmi_dai = {
	.name = "ce5300-hdmi-i2s",
	.playback = {
		.stream_name	= "HDMI Playback",
		.channels_min	= 2,
		.channels_max	= 2,
		.rates		= SNDRV_PCM_RATE_32000 | SNDRV_PCM_RATE_44100 |
				  SNDRV_PCM_RATE_48000,
		.formats	= SNDRV_PCM_FMTBIT_S16_LE | SNDRV_PCM_FMTBIT_S24_LE |
				  SNDRV_PCM_FMTBIT_S32_LE,
	},
};

static int ce5300_hdmi_probe(struct platform_device *pdev)
{
	/*
	 * Register the component + DAI only. NO ioremap, NO PCI, NO MMIO — see
	 * the banner. When the backend is RE'd it will likely want to find and
	 * map the HDMI-audio function (8086:2e5f / 8086:2e60) here, which must be
	 * done under supervision only (reading the wrong register hangs the SoC).
	 */
	return devm_snd_soc_register_component(&pdev->dev, &ce5300_hdmi_component,
					       &ce5300_hdmi_dai, 1);
}

static struct platform_driver ce5300_hdmi_driver = {
	.driver = {
		.name = "ce5300-hdmi",
	},
	.probe = ce5300_hdmi_probe,
};
module_platform_driver(ce5300_hdmi_driver);

MODULE_ALIAS("platform:ce5300-hdmi");
MODULE_DESCRIPTION("Intel CE5300 HDMI-audio ASoC platform/DAI skeleton (no-op; register map un-RE'd)");
MODULE_LICENSE("GPL");
