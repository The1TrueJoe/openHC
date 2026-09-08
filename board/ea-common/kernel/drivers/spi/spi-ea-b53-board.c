// SPDX-License-Identifier: GPL-2.0
/*
 * Board glue for the Control4 EA3 (Intel Atom CE5310).
 *
 * The EA3 has a Broadcom BCM53125 managed switch on SPI bus 0, chip-select 1.
 * Its two external ports are the unit's rear RJ45s; the on-SoC e1000 MAC hangs
 * off switch PORT 5 (not the IMP at port 8 — measured, see below) on a fixed
 * forced link, "internal fake phy" in the vendor's words. Without DSA the
 * primary jack works and the second one is dark: the stock switch config never
 * forwards port 1 to the CPU port.
 *
 * Bringing the switch up under DSA gives each jack its own netdev, which is the
 * point: the vendor's out-of-tree spi-bcm53125 driver never did.
 *
 * There is no device tree on this machine — CEFDK's bootlinux hands the kernel
 * no DTB — so the switch is described here in code. spi_register_board_info()
 * is the mechanism intended for that: the entry waits on a queue until a
 * controller with the matching bus_num registers, then the SPI core creates the
 * device. Registering at subsys_initcall puts us ahead of spi-pxa2xx-pci.
 *
 * DESCRIPTION MECHANISM: DSA in 7.1.8 accepts a port map from EITHER the
 * device's of_node OR its platform_data (as a struct dsa_chip_data) — see
 * dsa_switch_probe(): np ? dsa_switch_parse_of() : pdata ? dsa_switch_parse().
 * There is NO fwnode/software-node path, so a swnode graph would be ignored.
 * With no DTB on this machine, platform_data is the only option, and it still
 * works: dsa_chip_data lives in <linux/platform_data/dsa.h> and
 * dsa_switch_parse_ports() still reads its port_names[]/netdev[]. Only the
 * dsa_platform_data WRAPPER was removed, so we hand over the chip data directly.
 *
 * See board/ea3-v2/patches/linux/0003-*.patch in openHC for what is verified here
 * and what is not. In short: the bus, chip-select and the whole port map were
 * measured on a live unit; what is untested is DSA's bring-up of them.
 */

#include <linux/bits.h>
#include <linux/init.h>
#include <linux/string.h>
#include <linux/kernel.h>
#include <linux/pci.h>
#include <linux/spi/spi.h>
#include <asm/setup.h>              /* boot_command_line */
#include <linux/platform_data/b53.h>   /* struct b53_platform_data — what b53
                                       * actually reads dev->platform_data as;
                                       * its first member IS the dsa_chip_data. */
#include <linux/platform_data/dsa.h>   /* dsa_chip_data — MOVED here; it is
                                        * no longer in <net/dsa.h>, which is
                                        * exactly why the first version of
                                        * this file would not compile. */

/*
 * The SPI BUS NUMBER IS NOT 0, and it is not ours to choose. spi-pxa2xx-pci's
 * ce4100_spi_setup() does
 *     ssp->port_id = dev->devfn;
 * and pxa2xx_spi_probe() then does
 *     controller->bus_num = ssp->port_id;
 * so the bus number is the SPI function's PCI devfn. On the EA3 that function
 * is 0000:01:0b.4, giving devfn = (0x0b << 3) | 4 = 92 — the controller comes
 * up as spi92 and a board_info queued for bus 0 silently never matches. That is
 * measured, not theorised: with bus 0 hardcoded the kernel logged the switch as
 * "queued for spi0.1" forever while /sys/class/spi_master held only spi92.
 *
 * So derive it from the same place the driver does rather than hardcoding 92 —
 * that keeps this correct on any EA variant that puts the SPI function at a
 * different slot. EA_SWITCH_BUS below is only the fallback for the (impossible
 * in practice) case where the lookup fails.
 */
#define EA_SPI_DEVICE_ID	0x2e6a	/* Intel CE4100-compatible SPI function */
#define EA_SWITCH_BUS		0
#define EA_SWITCH_CS		1

/* Superseded by EA_SWITCH_HZ_VENDOR below — the vendor's actual clock HAS now
 * been recovered from their GPL source, so this guess is no longer used. */

/*
 * Port map for the BCM53125.
 *
 * port_names[] is indexed by switch port number. "cpu" marks the port wired to
 * the host MAC; every other named port becomes its own netdev.
 *
 * PORT 5 IS THE CPU PORT, NOT THE IMP AT 8. Measured on a live EA3 through the
 * vendor driver's register sysfs: link summary (page 0x01 reg 0x00) reads 0x24
 * — ports 2 and 5 — and those two ports' MIB counters are exact mirrors of
 * each other. Port 5's GMII override (page 0x00 reg 0x5d) reads 0x4b: forced
 * link, 1000M, full duplex, which is the fixed "internal fake phy" link the
 * vendor kernel announces. Port 8 shows no link and all-zero counters.
 *
 * PORT 2 is the primary jack, also measured: pushing 4 MB out of the box moved
 * 4,313,853 bytes into port 5 and 4,313,811 bytes out of port 2, with every
 * other port flat.
 *
 * PORT 1 is the second jack, measured by moving the cable. Note that it is
 * ISOLATED in the stock configuration: with a cable in it the port links and
 * receives (1.4 MB, 4565 multicast frames) but its TxOctets stays at exactly 0
 * and the host is unreachable. Bringing it up as a real netdev is precisely
 * what this DSA setup is for — the vendor never exposed it.
 */
#define EA_SWITCH_CPU_PORT	5

/* The SoC's GbE MAC, measured on a live EA3: 0000:01:0c.0, driver e1000,
 * netdev eth0. This is the device the CPU port's conduit hangs off. */
#define EA_E1000_DEVICE_ID	0x2e6e

/*
 * b53's chip-id enum (BCM53125_DEVICE_ID) lives in drivers/net/dsa/b53/b53_priv.h,
 * a PRIVATE header we cannot include from drivers/spi/. The value is part of the
 * platform_data ABI, so repeat it here rather than reaching across the tree.
 */
#define EA_BCM53125_CHIP_ID	0x53125

/*
 * NOTE THE TYPE. b53_spi_probe() does
 *     dev->pdata = spi->dev.platform_data;
 * and b53_switch_register() then reads pdata->chip_id and pdata->enabled_ports
 * — i.e. it interprets platform_data as a struct b53_platform_data, NOT as a
 * bare dsa_chip_data. Handing it a bare dsa_chip_data (which this file did at
 * first) means those two fields are read from whatever memory happens to follow
 * the struct, and probe dies:
 *     b53-switch spi92.1: probe with driver b53-switch failed with error -22
 * b53_platform_data keeps dsa_chip_data as its FIRST member precisely so the
 * same pointer works for both — see the comment in <linux/platform_data/b53.h>.
 *
 * chip_id and enabled_ports are deliberately left ZERO so b53 does everything
 * itself: b53_switch_register() falls back to b53_switch_detect() over SPI, and
 * b53_switch_init() then fills enabled_ports from its own per-chip table.
 *
 * An earlier version set chip_id = 0x53125 explicitly, reasoning that naming a
 * part we know is soldered down would give a louder failure. That was a mistake
 * worth recording: setting chip_id SKIPS detection, so the reassuring
 *     b53-switch spi92.1: found switch: BCM53125, rev 0
 * was just our own constant echoed back and proved nothing about whether SPI
 * reads work at all. With autodetect the message is real evidence — if it
 * appears, register reads over SPI genuinely work and any remaining problem is
 * downstream (the PHY scan); if probe fails with -EINVAL instead, SPI reads are
 * broken and that is the thing to fix. Do not "helpfully" hardcode it again.
 */
static struct b53_platform_data ea_switch_pdata = {
	/* .chip_id = 0 -> autodetect over SPI (see above).
	 * .enabled_ports = 0 -> b53 uses its own per-chip default. */
	.cd = {
		.port_names[1]			= "lan1",  /* isolated in stock cfg */
		.port_names[2]			= "lan2",  /* the primary jack      */
		.port_names[EA_SWITCH_CPU_PORT]	= "cpu",   /* to the SoC e1000      */
		/* .netdev[EA_SWITCH_CPU_PORT] is filled in at init — see below.
		 * It cannot be a static initialiser because it is a runtime
		 * pointer to the e1000's struct device. */
	},
};


/*
 * SPI PARAMETERS, TAKEN FROM CONTROL4'S OWN GPL DRIVER — not guessed.
 * Source: patches/c4_patches/0009-bcm53125-switch-support.patch, b53SpiInit():
 *
 *     // ss is cs1
 *     chip->select      = 1;
 *     chip->clock_div   = 1;      // no division: full SSP rate
 *     chip->frame_format = 0;     // Motorola
 *     chip->data_size   = 8;
 *
 * and spiMasterConfig(), which programs SSCR1 as:
 *
 *     data |= SSCR1_SPH_BIT;      // clock phase    = 1
 *     data |= SSCR1_SPO_BIT;      // clock polarity = 1
 *
 * SPH=1 + SPO=1 is CPHA=1 + CPOL=1, i.e. SPI_MODE_3.
 *
 * THIS IS THE BUG THAT COST THE MOST TIME. Every earlier attempt used
 * SPI_MODE_0, and the failure mode was maximally unhelpful: transfers complete
 * normally, the controller's IRQ counts up, and the switch simply never
 * answers, so b53_switch_detect() returns nothing and probe dies with a bare
 * -EINVAL. That looks identical to a wrong chip select, which sent us
 * sweeping CS 0..3 (all four failed, because the mode was wrong for all four).
 *
 * The same vendor function also writes the chip select into SSSR:
 *     data &= ~SSSR_FRM_BITS; data |= chip_select;
 * which is exactly what mainline's cs_assert() does for CE4100_SSP — confirming
 * patch 0008 (ssp->type = CE4100_SSP) independently.
 *
 * Clock: the vendor divides by 1, so it runs at the SSP's own rate. The PXA25x
 * SSP clocks at base/(2*(SCR+1)), and spi-pxa2xx-pci registers the base at
 * 3686400 Hz, so the fastest achievable is 1843200. Asking for exactly that
 * makes the driver pick SCR=0, matching clock_div = 1.
 */
#define EA_SWITCH_SPI_MODE	SPI_MODE_3
#define EA_SWITCH_HZ_VENDOR	1843200

static struct spi_board_info ea_spi_devices[] __initdata = {
	{
		.modalias	= "bcm53125",
		.bus_num	= EA_SWITCH_BUS,
		.chip_select	= EA_SWITCH_CS,
		.max_speed_hz	= EA_SWITCH_HZ_VENDOR,
		.mode		= EA_SWITCH_SPI_MODE,
		.platform_data	= &ea_switch_pdata,
	},
};

/*
 * OPT-IN, AND DELIBERATELY SO. A half-attached switch does not fail politely:
 * once b53 binds, the SoC MAC becomes the switch's CPU port and expects tagged
 * frames, so if the ports then fail to come up the machine has no usable
 * interface at all and drops off the network — which also breaks the netboot
 * RAM installer, the very thing used to repair a bad rootfs.
 *
 * Default off. Enable with `ohc.switch=1` on the kernel command line (for a
 * persistent boot, the cmdline in the CEFDK autoscript — see
 * tools/ohc-ea-takeover.py --root-cmdline). S40net's dsa_unbind fallback is the
 * second net underneath this one.
 */
static bool ea_switch_wanted(void)
{
	return strstr(boot_command_line, "ohc.switch=1") != NULL;
}

static int __init ea_b53_board_init(void)
{
	struct pci_dev *pdev, *spidev;
	int ret;

	if (!ea_switch_wanted()) {
		pr_info("ea-b53-board: switch off (pass ohc.switch=1 to enable)\n");
		return 0;
	}

	/*
	 * Resolve the CPU port's conduit. DSA's platform_data path does
	 *   dev_find_class(cd->netdev[cpu_port], "net")
	 * in dsa_port_parse(), so it wants the MAC's PARENT struct device and
	 * finds the net_device as a child of it. Leaving netdev[] NULL — which
	 * the first version of this file did — makes the CPU port permanently
	 * unresolvable, so the switch never attaches no matter how correct the
	 * rest of the port map is.
	 *
	 * The e1000 has not necessarily probed by subsys_initcall, and that is
	 * fine: dsa_port_parse() returns -EPROBE_DEFER while no net class device
	 * exists under it yet, and the deferred-probe machinery retries once
	 * e1000 has created eth0.
	 */
	pdev = pci_get_device(PCI_VENDOR_ID_INTEL, EA_E1000_DEVICE_ID, NULL);
	if (!pdev) {
		pr_warn("ea-b53-board: no e1000 at 8086:%04x — not registering the switch\n",
			EA_E1000_DEVICE_ID);
		return -ENODEV;
	}
	/*
	 * The reference from pci_get_device() is deliberately NOT dropped:
	 * ea_switch_chip is static and outlives this function, so the pointer
	 * must stay valid for the lifetime of the switch.
	 */
	ea_switch_pdata.cd.netdev[EA_SWITCH_CPU_PORT] = &pdev->dev;

	/*
	 * Derive the SPI bus number from the SPI function's devfn — see the
	 * comment at EA_SPI_DEVICE_ID. PCI is enumerated well before
	 * subsys_initcall, so this lookup succeeds here even though
	 * spi-pxa2xx-pci itself (a device_initcall) has not probed yet, which is
	 * exactly the ordering we need: the board_info must be on the queue
	 * before the controller registers.
	 */
	spidev = pci_get_device(PCI_VENDOR_ID_INTEL, EA_SPI_DEVICE_ID, NULL);
	if (spidev) {
		ea_spi_devices[0].bus_num = spidev->devfn;
		pci_dev_put(spidev);
	} else {
		pr_warn("ea-b53-board: no SPI function at 8086:%04x — falling back to bus %d\n",
			EA_SPI_DEVICE_ID, EA_SWITCH_BUS);
	}

	ret = spi_register_board_info(ea_spi_devices,
				      ARRAY_SIZE(ea_spi_devices));
	if (ret) {
		pr_warn("ea-b53-board: could not register bcm53125 at spi%d.%d: %d\n",
			ea_spi_devices[0].bus_num, EA_SWITCH_CS, ret);
		return ret;
	}

	pr_info("ea-b53-board: bcm53125 queued for spi%d.%d @ %u Hz mode %u\n",
		ea_spi_devices[0].bus_num, EA_SWITCH_CS,
		ea_spi_devices[0].max_speed_hz, ea_spi_devices[0].mode);
	return 0;
}

/*
 * subsys_initcall, not device_initcall: the board info has to be on the queue
 * before spi-pxa2xx-pci registers the controller, or the SPI core will have
 * nothing to match and the switch is never created.
 */
subsys_initcall(ea_b53_board_init);
