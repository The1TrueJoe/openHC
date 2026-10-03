// SPDX-License-Identifier: GPL-2.0
/*
 * Board glue for EA1 HDMI audio.
 *
 * CEFDK hands the kernel no DTB, so nothing describes the audio topology to the
 * driver model. On EA1 that topology is deliberately tiny — there is NO I2C
 * codec and NO mute GPIO (unlike ce5300-ea-board.c, which instantiates the
 * ADAU1451 on i2c-3 and wires its DAC-mute line). All EA1 needs is two software
 * platform devices:
 *
 *   "ce5300-hdmi"       - the HDMI-audio platform + cpu-DAI component
 *                         (bound by ce5300-hdmi.c)
 *   "ce5300-ea1-audio"  - the ASoC card (bound by ce5300-ea1.c)
 *
 * This file exists only to register those two devices; it touches no hardware
 * and references no snd-soc symbol, so as a module its initcall becomes
 * module_init and the two component/machine modules bind the devices by name
 * afterwards (deferred probe makes the order forgiving).
 *
 * Intentionally NOT modelled on ce5300-ea-board.c's i2c/gpio glue: EA1 has no
 * I2C codec to instantiate and no DAC-mute line to look up.
 */
#include <linux/init.h>
#include <linux/module.h>
#include <linux/platform_device.h>

static struct platform_device *ea1_hdmi_dev;
static struct platform_device *ea1_card_dev;

static int __init ea1_audio_board_init(void)
{
	ea1_hdmi_dev = platform_device_register_simple("ce5300-hdmi", -1,
						       NULL, 0);
	if (IS_ERR(ea1_hdmi_dev)) {
		pr_err("ea1-audio: cannot register the HDMI component device\n");
		ea1_hdmi_dev = NULL;
		return 0;	/* not fatal: a board must still boot without audio */
	}

	ea1_card_dev = platform_device_register_simple("ce5300-ea1-audio", -1,
						       NULL, 0);
	if (IS_ERR(ea1_card_dev)) {
		pr_err("ea1-audio: cannot register the card device\n");
		ea1_card_dev = NULL;
	}
	return 0;
}

static void __exit ea1_audio_board_exit(void)
{
	platform_device_unregister(ea1_card_dev);
	platform_device_unregister(ea1_hdmi_dev);
}

module_init(ea1_audio_board_init);
module_exit(ea1_audio_board_exit);

MODULE_DESCRIPTION("Control4 EA1 HDMI-audio board glue (platform-device registration)");
MODULE_LICENSE("GPL");
