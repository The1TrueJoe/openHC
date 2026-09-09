// SPDX-License-Identifier: GPL-2.0
/*
 * openHC — Control4 HC-800 board glue: the front-panel LEDs and the ID button.
 *
 * The HC-800 is an x86 PC with no device tree and no ACPI description of any of
 * this, so nothing enumerates the panel on its own. The vendor solves that with
 * a board file in its own kernel; this is the same idea, kept to the two things
 * that are actually wired and confirmed.
 *
 * WHERE THE NUMBERS COME FROM. Not guessed: read out of the vendor kernel's own
 * struct gpio_led and gpio_keys_button arrays, then corroborated twice on live
 * hardware — an EBUSY/ENODEV probe of every line on the running vendor OS, and
 * the ICH's own GPIO_USE_SEL register (0x1f40f7cd), which says which pins the
 * BIOS wired as GPIO. All three agree. See board.env for the full table.
 *
 * Every line used here is in USE_SEL, so mainline gpio-ich accepts them and we
 * do NOT need the vendor's trick of forcing a pin into GPIO mode by rewriting
 * that register — it only ever did that for pins 4 and 5, which we do not use.
 *
 * Copyright (C) 2026 openHC
 */

#define pr_fmt(fmt) "ohc-hc800: " fmt

#include <linux/dmi.h>
#include <linux/gpio/machine.h>
#include <linux/gpio_keys.h>
#include <linux/input.h>
#include <linux/leds.h>
#include <linux/module.h>
#include <linux/platform_device.h>

/*
 * The gpiochip label, not its number: gpio-ich's base is dynamic on a modern
 * kernel and the vendor's own notes record it moving between versions.
 */
#define ICH_CHIP "gpio_ich"

/*
 * Four physical LEDs, six channels. {8,9,10} are the three dies of ONE
 * tri-colour wifi indicator; {26,27,28} are three separate status LEDs. All
 * active-high.
 *
 * c4::power keeps the vendor's default-on trigger: on stock it is a power
 * indicator rather than a software LED, and its own /etc/init.d/gpio pointedly
 * does not expose it.
 */
static const struct gpio_led hc800_leds[] = {
	{ .name = "hc800:red:wifi",      .default_trigger = NULL },
	{ .name = "hc800:yellow:wifi",   .default_trigger = NULL },
	{ .name = "hc800:blue:wifi",     .default_trigger = "timer" },
	{ .name = "hc800:green:data",    .default_trigger = NULL },
	{ .name = "hc800:green:network", .default_trigger = NULL },
	{ .name = "hc800:green:power",   .default_trigger = "default-on" },
};

static const struct gpio_led_platform_data hc800_led_pdata = {
	.num_leds = ARRAY_SIZE(hc800_leds),
	.leds     = hc800_leds,
};

/*
 * Descriptors by index, in the same order as the array above. GPIO_LOOKUP_IDX
 * rather than gpio_led.gpio, which is the deprecated integer interface.
 */
static struct gpiod_lookup_table hc800_led_gpios = {
	.dev_id = "leds-gpio",
	.table = {
		GPIO_LOOKUP_IDX(ICH_CHIP,  8, NULL, 0, GPIO_ACTIVE_HIGH),
		GPIO_LOOKUP_IDX(ICH_CHIP,  9, NULL, 1, GPIO_ACTIVE_HIGH),
		GPIO_LOOKUP_IDX(ICH_CHIP, 10, NULL, 2, GPIO_ACTIVE_HIGH),
		GPIO_LOOKUP_IDX(ICH_CHIP, 26, NULL, 3, GPIO_ACTIVE_HIGH),
		GPIO_LOOKUP_IDX(ICH_CHIP, 27, NULL, 4, GPIO_ACTIVE_HIGH),
		GPIO_LOOKUP_IDX(ICH_CHIP, 28, NULL, 5, GPIO_ACTIVE_HIGH),
		{ },
	},
};

/*
 * The ID/setup button. ACTIVE LOW, unlike the LEDs, and polled because the ICH
 * line has no usable interrupt. 40 ms poll and 20 ms debounce are the vendor's
 * own figures.
 *
 * KEY_F5 is what the vendor emits (code 63, confirmed against its running
 * /proc/bus/input/devices). Odd, but keeping it means anything written for a
 * stock unit still works.
 */
static struct gpio_keys_button hc800_buttons[] = {
	{
		.code              = KEY_F5,
		.desc              = "id_button",
		.type              = EV_KEY,
		.active_low        = 1,
		.debounce_interval = 20,
	},
};

static const struct gpio_keys_platform_data hc800_keys_pdata = {
	.buttons       = hc800_buttons,
	.nbuttons      = ARRAY_SIZE(hc800_buttons),
	.poll_interval = 40,
	.name          = "hc800-id-button",
};

static struct gpiod_lookup_table hc800_key_gpios = {
	.dev_id = "gpio-keys-polled",
	.table = {
		GPIO_LOOKUP_IDX(ICH_CHIP, 2, NULL, 0, GPIO_ACTIVE_LOW),
		{ },
	},
};

static struct platform_device *hc800_leds_dev;
static struct platform_device *hc800_keys_dev;

/*
 * Only this board. The panel wiring is specific to it, and driving another
 * machine's ICH lines because a kernel happened to be built with this in would
 * be a genuinely bad outcome.
 *
 * Strings read off the live unit: sys_vendor "Lite-On Tech.", product_name
 * "HC800". DMI_MATCH is a substring test, so "Lite-On" is deliberate.
 */
static const struct dmi_system_id hc800_dmi[] = {
	{
		.matches = {
			DMI_MATCH(DMI_SYS_VENDOR, "Lite-On"),
			DMI_MATCH(DMI_PRODUCT_NAME, "HC800"),
		},
	},
	{ }
};
MODULE_DEVICE_TABLE(dmi, hc800_dmi);

static int __init ohc_hc800_init(void)
{
	if (!dmi_check_system(hc800_dmi))
		return -ENODEV;

	gpiod_add_lookup_table(&hc800_led_gpios);
	hc800_leds_dev = platform_device_register_data(NULL, "leds-gpio",
						       PLATFORM_DEVID_NONE,
						       &hc800_led_pdata,
						       sizeof(hc800_led_pdata));
	if (IS_ERR(hc800_leds_dev)) {
		gpiod_remove_lookup_table(&hc800_led_gpios);
		pr_warn("no front-panel LEDs: %ld\n", PTR_ERR(hc800_leds_dev));
		hc800_leds_dev = NULL;
	}

	gpiod_add_lookup_table(&hc800_key_gpios);
	hc800_keys_dev = platform_device_register_data(NULL, "gpio-keys-polled",
						       PLATFORM_DEVID_NONE,
						       &hc800_keys_pdata,
						       sizeof(hc800_keys_pdata));
	if (IS_ERR(hc800_keys_dev)) {
		gpiod_remove_lookup_table(&hc800_key_gpios);
		pr_warn("no ID button: %ld\n", PTR_ERR(hc800_keys_dev));
		hc800_keys_dev = NULL;
	}

	/* Either half failing is survivable; both failing means say nothing. */
	if (hc800_leds_dev || hc800_keys_dev)
		pr_info("front panel: %s%s\n",
			hc800_leds_dev ? "6 LEDs " : "",
			hc800_keys_dev ? "ID button" : "");
	return 0;
}

static void __exit ohc_hc800_exit(void)
{
	if (hc800_keys_dev) {
		platform_device_unregister(hc800_keys_dev);
		gpiod_remove_lookup_table(&hc800_key_gpios);
	}
	if (hc800_leds_dev) {
		platform_device_unregister(hc800_leds_dev);
		gpiod_remove_lookup_table(&hc800_led_gpios);
	}
}

/*
 * late_initcall: gpio-ich is spawned by the lpc_ich MFD, so the chip does not
 * exist at the usual device_initcall point and every lookup here would fail.
 */
late_initcall(ohc_hc800_init);
module_exit(ohc_hc800_exit);

MODULE_LICENSE("GPL v2");
MODULE_DESCRIPTION("Control4 HC-800 front panel (LEDs + ID button)");
