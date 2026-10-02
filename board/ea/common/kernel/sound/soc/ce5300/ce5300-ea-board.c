// SPDX-License-Identifier: GPL-2.0
/*
 * Board glue for EA-series audio.
 *
 * CEFDK's bootlinux hands the kernel no DTB, so nothing describes the audio
 * hardware to the driver model: the ADAU1451 has to be instantiated on its I2C
 * bus by hand, and the ASoC card needs a platform device to hang off. Without
 * this file both drivers load, bind to nothing, and the box comes up with no
 * sound card and no error -- which looks exactly like a broken codec.
 *
 * Same shape as leds-ea-board.c and spi-ea-b53-board.c: board description in C
 * because there is nowhere else to put it. A DTB would replace all three.
 */
#include <linux/gpio/machine.h>
#include <linux/i2c.h>
#include <linux/init.h>
#include <linux/module.h>
#include <linux/platform_device.h>

/*
 * The codec lives at 0x38 on the CE5300's FOURTH I2C controller. That bus only
 * exists with patches/linux/0001 (CE4100_PCI_I2C_DEVS 3 -> 4); mainline creates
 * three adapters and the codec is then unreachable. If this lookup fails, check
 * that patch before suspecting the codec.
 */
#define EA_CODEC_I2C_BUS	3
#define EA_CODEC_I2C_ADDR	0x38

/*
 * DAC soft mute, vendor global GPIO 61 on the CE5300 controller. Active high =
 * muted. Consumer id "mute" matches devm_gpiod_get_optional() in the codec.
 *
 * gpio-intelce sets chip.base = 0 and exposes all 128 lines, so the vendor's
 * numbering and ours agree and 61 can be used directly.
 */
static struct gpiod_lookup_table ea_audio_gpios = {
	.dev_id = "3-0038",		/* i2c bus 3, addr 0x38 */
	.table = {
		GPIO_LOOKUP("intelce-gpio", 61, "mute", GPIO_ACTIVE_HIGH),
		{ },
	},
};

static struct i2c_board_info ea_codec_info __initdata = {
	I2C_BOARD_INFO("adau1451", EA_CODEC_I2C_ADDR),
};

static struct i2c_client *ea_codec_client;
static struct platform_device *ea_card_dev;

static int __init ea_audio_board_init(void)
{
	struct i2c_adapter *adap;

	adap = i2c_get_adapter(EA_CODEC_I2C_BUS);
	if (!adap) {
		pr_info("ea-audio: no i2c-%d; audio disabled (is the 4-controller i2c patch applied?)\n",
			EA_CODEC_I2C_BUS);
		return 0;	/* not fatal: a board without audio must still boot */
	}

	gpiod_add_lookup_table(&ea_audio_gpios);

	ea_codec_client = i2c_new_client_device(adap, &ea_codec_info);
	i2c_put_adapter(adap);
	if (IS_ERR(ea_codec_client)) {
		pr_warn("ea-audio: codec not present at 0x%02x on i2c-%d\n",
			EA_CODEC_I2C_ADDR, EA_CODEC_I2C_BUS);
		gpiod_remove_lookup_table(&ea_audio_gpios);
		ea_codec_client = NULL;
		return 0;
	}

	/*
	 * The card is registered even though the codec may not have probed yet;
	 * ASoC defers the link until every component shows up, which is the
	 * whole point of its component framework.
	 */
	ea_card_dev = platform_device_register_simple("ce5300-ea-audio", -1,
						      NULL, 0);
	if (IS_ERR(ea_card_dev)) {
		pr_err("ea-audio: cannot register the card device\n");
		ea_card_dev = NULL;
	}
	return 0;
}

/*
 * late_initcall: i2c-pxa-pci is a device_initcall and its adapters must exist
 * before i2c_get_adapter() can find bus 3. Registering earlier finds no adapter
 * and silently gives up -- the same ordering trap leds-ea-board.c documents.
 */
late_initcall(ea_audio_board_init);

MODULE_DESCRIPTION("Control4 EA-series audio board glue (codec + card instantiation)");
MODULE_LICENSE("GPL");
