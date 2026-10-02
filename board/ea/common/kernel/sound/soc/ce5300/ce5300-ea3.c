// SPDX-License-Identifier: GPL-2.0
/*
 * ASoC machine driver for the Control4 EA-series audio path:
 *
 *     CE5300 I2S (ce5300-i2s, PCI 8086:2e60)  --I2S-->  ADAU1451 SigmaDSP  --> analog out
 *
 * Replaces Control4's ninjago-smd-dsp.c, keeping only what is load-bearing.
 * Dropped from the vendor version: the sysclk pick in hw_params (our codec
 * driver does not act on sysclk), the .playback_count = 4 ASoC core patch (we
 * have one stream, not four), and every DSP tone/EQ control (their addresses
 * live in an Analog Devices header that may not be redistributed -- see
 * adau1451-c4.c).
 *
 * The codec is the I2S MASTER (the vendor sets CBM_CFM and the CE5300 render
 * path exposes no clock-direction control at all, which is consistent with the
 * SoC side being wired as a permanent slave). So the SoC generates neither
 * bit clock nor word select, and this driver must not try to.
 *
 * There is no device tree on these boards -- CEFDK boots the kernel directly --
 * so the card is instantiated from a platform device registered by board code
 * rather than from DT, and the codec is named by its I2C address.
 */
#include <linux/module.h>
#include <linux/platform_device.h>
#include <sound/soc.h>

#define CARD_NAME	"openhc-ea"

SND_SOC_DAILINK_DEFS(hifi,
	DAILINK_COMP_ARRAY(COMP_CPU("ce5300-i2s")),
	/*
	 * "adau1451.<bus>-<addr>" is the I2C device name. Bus 3 is the CE5300's
	 * FOURTH I2C controller, which mainline does not enumerate by default --
	 * see the i2c-pxa-pci patch. Without that patch this link never probes,
	 * because the codec simply is not on any bus the kernel created.
	 */
	DAILINK_COMP_ARRAY(COMP_CODEC("adau1451.3-0038", "adau1451-ch0")),
	DAILINK_COMP_ARRAY(COMP_PLATFORM("ce5300-i2s")));

static struct snd_soc_dai_link ea_dai_links[] = {
	{
		.name		= "ADAU1451",
		.stream_name	= "Playback",
		/*
		 * CBP_CFP = codec is both bit-clock and frame PROVIDER, i.e. the
		 * old CBM_CFM. The provider/consumer names are what this kernel
		 * has (CBM_CFM is gone). Note gcc suggests CBP_CFC on the error,
		 * which is frame CONSUMER -- wrong, and it would leave the SoC
		 * trying to drive a frame clock the codec is already driving.
		 */
		.dai_fmt	= SND_SOC_DAIFMT_I2S | SND_SOC_DAIFMT_NB_NF |
				  SND_SOC_DAIFMT_CBP_CFP,
		/*
		 * No .playback_only here even though this link is exactly that:
		 * the field has been renamed more than once across recent kernels
		 * (dpcm_playback -> playback_only), and neither DAI declares a
		 * capture stream, so ASoC works it out anyway. Not worth a build
		 * break for a hint.
		 */
		SND_SOC_DAILINK_REG(hifi),
	},
};

static struct snd_soc_card ea_card = {
	.name		= CARD_NAME,
	.owner		= THIS_MODULE,
	.dai_link	= ea_dai_links,
	.num_links	= ARRAY_SIZE(ea_dai_links),
};

static int ea_audio_probe(struct platform_device *pdev)
{
	ea_card.dev = &pdev->dev;
	return devm_snd_soc_register_card(&pdev->dev, &ea_card);
}

static struct platform_driver ea_audio_driver = {
	.driver = {
		.name = "ce5300-ea-audio",
		.pm   = &snd_soc_pm_ops,
	},
	.probe = ea_audio_probe,
};
module_platform_driver(ea_audio_driver);

MODULE_DESCRIPTION("Control4 EA-series (CE5300 + ADAU1451) ASoC machine driver");
MODULE_ALIAS("platform:ce5300-ea-audio");
MODULE_LICENSE("GPL");
