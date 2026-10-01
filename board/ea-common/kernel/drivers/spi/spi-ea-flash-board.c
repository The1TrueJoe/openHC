// SPDX-License-Identifier: GPL-2.0
/*
 * Board glue: declare the Control4 EA boot SPI-NOR flash so openHC gets an MTD.
 *
 * The boot flash (an S25FL127S — JEDEC 0x012018, a 16 MiB S25FL128S-class part)
 * is where CEFDK, its Master Flash Header and the openHC boot autoscript live.
 * Until Linux binds it there is no /dev/mtd0, which is why openHC could neither
 * rewrite its own autoscript for a clean in-place kernel install nor restore the
 * stock firmware from software — the two things an MTD unlocks.
 *
 * It hangs off the SAME CE5300 SPI controller as the BCM switch (PCI 8086:2e6a,
 * which pxa2xx-spi-pci brings up as spiNN where NN is the function's devfn), but
 * on CHIP-SELECT 0 — the switch is CS 1. Control4 drove it with a bespoke pair
 * (ce5xx_spi_flash + nmyx25); this part is standard, so mainline's spi-nor
 * handles it and we only have to DESCRIBE the device. There is no device tree on
 * this machine (CEFDK's bootlinux passes no DTB), so spi_register_board_info()
 * is the mechanism, exactly as for the switch in spi-ea-b53-board.c.
 *
 * Registered UNCONDITIONALLY, unlike the switch (which is gated behind
 * ohc.switch=1): the flash is always present, reading it is harmless, and an MTD
 * is a prerequisite for both the in-place installer and a software stock-restore.
 *
 * modalias "spi-nor" is the generic non-DT entry in the spi-nor driver's
 * spi_device_id table; the driver then autodetects the chip over SPI by its
 * JEDEC id, so no per-chip name is needed here.
 *
 * Mode 3 at 1.8 MHz mirrors the switch's settings, which are the ones PROVEN to
 * work on this CE4100-type SSP: spi-ea-b53-board.c records at length that
 * SPI_MODE_0 completes transfers but the device never answers on this
 * controller. The SSP base is 3.6864 MHz and divides by 1, so 1843200 is the
 * fastest it reaches; the flash is happy slower, and correctness beats speed on
 * a part we are going to write our own boot header into.
 */
#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/spi/spi.h>

#define EA_SPI_DEVICE_ID	0x2e6a	/* Intel CE4100-compatible SPI function */
#define EA_FLASH_CS		0	/* system boot flash; switch is CS 1 */
#define EA_FLASH_HZ		1843200	/* SSP base / 1, as the switch uses */

static struct spi_board_info ea_flash_devices[] __initdata = {
	{
		.modalias	= "spi-nor",
		.bus_num	= 0,		/* replaced with the real devfn below */
		.chip_select	= EA_FLASH_CS,
		.max_speed_hz	= EA_FLASH_HZ,
		.mode		= SPI_MODE_3,
	},
};

static int __init ea_flash_board_init(void)
{
	struct pci_dev *spidev;
	int ret;

	/*
	 * The SPI bus number is the SPI function's PCI devfn (see the long note
	 * in spi-ea-b53-board.c): spi-pxa2xx-pci sets controller->bus_num from
	 * it, so a board_info queued for any other number never matches. PCI is
	 * enumerated well before subsys_initcall, so this resolves here even
	 * though the controller driver (device_initcall) has not probed yet —
	 * which is the ordering we need, so the info is on the queue first.
	 */
	spidev = pci_get_device(PCI_VENDOR_ID_INTEL, EA_SPI_DEVICE_ID, NULL);
	if (!spidev) {
		pr_warn("ea-flash-board: no SPI function at 8086:%04x — no MTD\n",
			EA_SPI_DEVICE_ID);
		return -ENODEV;
	}
	ea_flash_devices[0].bus_num = spidev->devfn;
	pci_dev_put(spidev);

	ret = spi_register_board_info(ea_flash_devices,
				      ARRAY_SIZE(ea_flash_devices));
	if (ret) {
		pr_warn("ea-flash-board: could not register spi-nor at spi%d.%d: %d\n",
			ea_flash_devices[0].bus_num, EA_FLASH_CS, ret);
		return ret;
	}

	pr_info("ea-flash-board: boot SPI-NOR queued for spi%d.%d @ %u Hz mode 3\n",
		ea_flash_devices[0].bus_num, EA_FLASH_CS, EA_FLASH_HZ);
	return 0;
}

/*
 * subsys_initcall, like the switch: the board info must be on the queue before
 * spi-pxa2xx-pci (device_initcall) registers the controller, or the SPI core has
 * nothing to match and the flash device is never created.
 */
subsys_initcall(ea_flash_board_init);

MODULE_DESCRIPTION("Control4 EA boot SPI-NOR flash board glue");
MODULE_LICENSE("GPL");
