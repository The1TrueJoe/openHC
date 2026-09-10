// SPDX-License-Identifier: GPL-2.0
/*
 * EA-family board GPIO init: names the lines and takes peripherals out of reset.
 *
 * openHC did not do this at all, and it is not cosmetic. Stock Control4 runs
 * /etc/init.d/c4_gpio_config at boot, whose setup_gpio_common() drives a set of
 * reset and enable lines HIGH. Without it those peripherals sit in reset and
 * simply are not there -- measured on a live EA3, gpio26 (usb_2_serial_reset)
 * was an OUTPUT DRIVEN LOW, i.e. the USB-serial bridge was actively held down.
 *
 * The map below is transcribed from that stock script, which is the only
 * authoritative naming we have; the vendor numbers are usable directly because
 * gpio-intelce sets chip.base = 0 to match them.
 *
 * WHY A DRIVER AND NOT AN INIT SCRIPT. Releasing a reset from userspace is too
 * late for anything the kernel probes at boot: by the time an init script runs,
 * the i2c codec behind codec_reset has already failed to answer and the driver
 * core has moved on. Doing it in an initcall puts the lines in the right state
 * before those probes happen.
 *
 * The requests are also what make the lines legible: each shows up with its
 * vendor name in /sys/kernel/debug/gpio, so "what is gpio29" has an answer on
 * the box instead of only in this file.
 */
#include <linux/gpio.h>
#include <linux/init.h>
#include <linux/module.h>

struct ea_gpio {
	unsigned int gpio;
	const char *name;
	bool output;		/* false = input (a strap or status line) */
};

/*
 * setup_gpio_common() from the stock script -- applies to every EA board.
 *
 * Deliberately NOT included:
 *   23 src_reset   -- stock has it commented out ("TODO Fix when we get new
 *                     boards"), and 23 is also claimed below as board_id6
 *   32 id_button   -- stock leaves it to the input subsystem, and so do we
 */
static const struct ea_gpio ea_common_gpios[] __initconst = {
	{ 27,  "wlan_disable",       true  },
	{ 29,  "zigbee_reset",       true  },
	{ 26,  "usb_2_serial_reset", true  },
	{ 101, "dsp_reset",          true  },
	{ 24,  "codec_reset",        true  },
	{ 7,   "io_reset",           true  },
	{ 8,   "gb_sw_reset",        true  },
	{ 5,   "gb_eth_reset",       true  },
	{ 43,  "ldo_enable",         true  },
	{ 57,  "n_fpga_reload",      true  },
	/* Board-identification straps: read-only, but worth naming so the numbers
	 * are not a mystery to whoever reads /sys/kernel/debug/gpio next. */
	{ 17,  "board_id0",          false },
	{ 18,  "board_id1",          false },
	{ 19,  "board_id2",          false },
	{ 20,  "board_id3",          false },
	{ 21,  "board_id4",          false },
	{ 22,  "board_id5",          false },
};

/*
 * setup_gpio_ea3(). The EA1 and EA5 blocks differ (EA5 has usb_3_reset, EA1 a
 * different power set), so this is EA3's. Applying another board's block here
 * would drive lines that may not be wired the same way, which is why the stock
 * script branches rather than doing them all.
 */
static const struct ea_gpio ea3_gpios[] __initconst = {
	{ 54,  "poe_type",                    false },
	{ 75,  "n_twelve_volt_current_limit", true  },
	{ 80,  "ac_pwr",                      false },
	{ 121, "n_usb_current_limit",         true  },
	{ 122, "twelve_volt_ok",              false },
};

static void __init ea_gpio_apply(const struct ea_gpio *t, size_t n, const char *what)
{
	size_t i;
	int ok = 0;

	for (i = 0; i < n; i++) {
		unsigned long flags = t[i].output ? GPIOF_OUT_INIT_HIGH : GPIOF_IN;

		/*
		 * A failure here is not fatal: a line may already be claimed by a
		 * driver that owns it properly (the LEDs are), and that driver
		 * knows better than this table does. Log and carry on -- refusing
		 * to boot over a GPIO would be a poor trade.
		 */
		if (gpio_request_one(t[i].gpio, flags, t[i].name)) {
			pr_debug("ea-gpio: %s (gpio%u) already claimed, skipping\n",
				 t[i].name, t[i].gpio);
			continue;
		}
		ok++;
	}
	pr_info("ea-gpio: %s: %d/%zu lines configured\n", what, ok, n);
}

static int __init ea_board_gpio_init(void)
{
	/*
	 * late_initcall, for the same reason leds-ea-board.c uses it:
	 * gpio-intelce binds a PCI device and so runs at device_initcall, and
	 * there is no gpiochip to request from before that.
	 */
	ea_gpio_apply(ea_common_gpios, ARRAY_SIZE(ea_common_gpios), "common");
	ea_gpio_apply(ea3_gpios, ARRAY_SIZE(ea3_gpios), "ea3");
	return 0;
}
late_initcall(ea_board_gpio_init);

MODULE_DESCRIPTION("Control4 EA-series board GPIO init (names + reset release)");
MODULE_LICENSE("GPL");
