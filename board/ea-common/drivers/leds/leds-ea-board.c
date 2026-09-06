// SPDX-License-Identifier: GPL-2.0
/*
 * Front-panel LEDs for the Control4 EA boards (Intel Atom CE5300).
 *
 * The EA carries six GPIO-backed LEDs. There is no device tree on this machine
 * — CEFDK's bootlinux hands the kernel no DTB — so they are described here as
 * leds-gpio platform data, the same approach as the i2c/spi board glue.
 *
 * WHERE THE MAP COMES FROM. Control4's ninjago_platform code is absent from the
 * GPL drop, so the table was recovered from the STOCK KERNEL BINARY instead
 * (backups/ea3/stock-kernel-container.img): the six LED name strings are
 * contiguous, the pointers to them form a 16-byte-stride struct gpio_led[]
 * array, and the gpio_led_platform_data in front of it reads num_leds = 6.
 * Nothing was guessed and nothing was toggled on live hardware to find it —
 * several nearby lines are resets (NIC, switch, codec) that must not be poked.
 *
 * NOTE THE GPIO NUMBERS: 99, 100, 102 are far above the 12 lines mainline's
 * gpio-sodaville exposes for this controller. That is exactly why the LEDs were
 * unreachable before, and why openHC replaces it with gpio-intelce (128 lines,
 * ea-common/drivers/gpio/gpio-intelce.c). This file is useless without it.
 *
 * ACTIVE LOW: the two 4-ball LEDs carry flags = 1 in the stock table
 * (GPIO_ACTIVE_LOW). The four warn/network LEDs are active high.
 */

#include <linux/gpio.h>
#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/leds.h>
#include <linux/platform_device.h>

/* Recovered verbatim from the stock kernel's gpio_led[] (num_leds = 6). */
static const struct gpio_led ea_leds[] = {
	{ .name = "c4::network",    .gpio = 15,  .active_low = 0 },
	{ .name = "warn::red",      .gpio = 99,  .active_low = 0 },
	{ .name = "warn::yellow",   .gpio = 16,  .active_low = 0 },
	{ .name = "warn::blue",     .gpio = 100, .active_low = 0 },
	{ .name = "c4::4ball_red",  .gpio = 102, .active_low = 1 },
	{ .name = "c4::4ball_blue", .gpio = 34,  .active_low = 1 },
};

static const struct gpio_led_platform_data ea_leds_pdata = {
	.num_leds = ARRAY_SIZE(ea_leds),
	.leds     = ea_leds,
};

static struct platform_device *ea_leds_dev;

static int __init ea_leds_init(void)
{
	ea_leds_dev = platform_device_register_data(NULL, "leds-gpio",
						    PLATFORM_DEVID_NONE,
						    &ea_leds_pdata,
						    sizeof(ea_leds_pdata));
	if (IS_ERR(ea_leds_dev)) {
		pr_err("ea-leds: cannot register leds-gpio: %ld\n",
		       PTR_ERR(ea_leds_dev));
		return PTR_ERR(ea_leds_dev);
	}

	pr_info("ea-leds: %zu front-panel LEDs registered\n", ARRAY_SIZE(ea_leds));
	return 0;
}

/*
 * late_initcall, NOT subsys_initcall: leds-gpio has to resolve real GPIO
 * numbers, so gpio-intelce must already have registered its gpiochip. The SPI
 * board glue registers early because spi_register_board_info() only queues;
 * this one actually consumes GPIOs, so it has to run after the provider.
 */
late_initcall(ea_leds_init);
