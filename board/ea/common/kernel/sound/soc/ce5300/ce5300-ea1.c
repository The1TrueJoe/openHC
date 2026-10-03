// SPDX-License-Identifier: GPL-2.0
/*
 * ASoC machine driver for the Control4 EA1 audio path:
 *
 *     CE5300 HDMI audio (ce5300-hdmi)  --internal-->  integrated HDMI transmitter
 *
 * EA1's counterpart to ce5300-ea3.c. The crucial difference from EA3: EA1 has
 * NO ADAU1451 SigmaDSP, NO I2C codec, and NO DAC soft-mute GPIO. There is no
 * external codec at all — the "codec" is the SoC's integrated HDMI transmitter,
 * which this skeleton represents with the ASoC core's built-in snd-soc-dummy-dai
 * (COMP_DUMMY). When the real HDMI backend is reverse-engineered (see
 * HDMI-AUDIO-RE.md) this dummy link should become the generic hdmi-codec shim
 * so EDID/ELD and jack state flow through; until then dummy keeps the card
 * buildable with zero extra config or glue.
 *
 * As with EA3 there is no device tree — CEFDK boots the kernel directly — so the
 * card is instantiated from a platform device ("ce5300-ea1-audio") registered by
 * board code (ce5300-ea1-board.c) rather than from DT. That board file also
 * registers the "ce5300-hdmi" platform device the cpu/platform component binds.
 * Deliberately NO I2C codec instantiation and NO gpiod lookup table here or in
 * the glue — EA1 has neither.
 */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <sound/soc.h>

#define CARD_NAME	"openhc-ea1"

SND_SOC_DAILINK_DEFS(hdmi,
	DAILINK_COMP_ARRAY(COMP_CPU("ce5300-hdmi-i2s")),
	/*
	 * snd-soc-dummy-dai: EA1 has no external codec. The HDMI transmitter is
	 * on the SoC and (until the backend is RE'd) exposes no ASoC codec of its
	 * own, so the link's codec side is the ASoC core's always-present dummy.
	 * Swap COMP_DUMMY() for COMP_CODEC("hdmi-audio-codec", "i2s-hifi") once an
	 * hdmi-codec device is registered — see the fragment and HDMI-AUDIO-RE.md.
	 */
	DAILINK_COMP_ARRAY(COMP_DUMMY()),
	DAILINK_COMP_ARRAY(COMP_PLATFORM("ce5300-hdmi")));

static struct snd_soc_dai_link ea1_dai_links[] = {
	{
		.name		= "HDMI",
		.stream_name	= "HDMI Playback",
		/*
		 * The HDMI transmitter block is clocked by the SoC, and the
		 * HDMI-audio skeleton's cpu DAI sets no clocks; dummy imposes
		 * nothing. No dai_fmt is forced for the same reason ce5300-ea3
		 * documents around the provider/consumer rename: neither DAI
		 * declares a clock role, so leaving dai_fmt unset avoids pinning
		 * a direction the real backend may need to choose later.
		 */
		SND_SOC_DAILINK_REG(hdmi),
	},
};

static struct snd_soc_card ea1_card = {
	.name		= CARD_NAME,
	.owner		= THIS_MODULE,
	.dai_link	= ea1_dai_links,
	.num_links	= ARRAY_SIZE(ea1_dai_links),
};

static int ea1_audio_probe(struct platform_device *pdev)
{
	ea1_card.dev = &pdev->dev;
	return devm_snd_soc_register_card(&pdev->dev, &ea1_card);
}

static struct platform_driver ea1_audio_driver = {
	.driver = {
		.name = "ce5300-ea1-audio",
		.pm   = &snd_soc_pm_ops,
	},
	.probe = ea1_audio_probe,
};
module_platform_driver(ea1_audio_driver);

MODULE_DESCRIPTION("Control4 EA1 (CE5300 HDMI audio) ASoC machine driver");
MODULE_ALIAS("platform:ce5300-ea1-audio");
MODULE_LICENSE("GPL");
