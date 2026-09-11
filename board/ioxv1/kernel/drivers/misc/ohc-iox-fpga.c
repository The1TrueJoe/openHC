// SPDX-License-Identifier: GPL-2.0
/*
 * Control4 IO Extender V1 ("hammer") — Xilinx slave-serial FPGA loader.
 *
 * This board's four RS-232 ports and eight IR outputs are registers inside an
 * FPGA on async EMIF CS1, and that part comes out of reset BLANK. U-Boot does
 * not program it; the vendor's kernel does, from c4fpga.ko. So until a
 * bitstream is clocked in, those twelve pieces of IO do not exist — reading
 * their window returns nothing and an ns16550a probed against it finds no UART.
 *
 * That is also why the serial and IR nodes are CHILDREN of this one in the
 * device tree. Probed at boot as siblings they would autoconfigure against a
 * blank part, find nothing, and be gone before any loader ran. This driver
 * populates them only once DONE reads high, which is the same ordering the
 * vendor gets out of module dependencies.
 *
 * The sequence is stock Xilinx slave-serial, and Control4 published their own
 * implementation of it under GPL in U-Boot (cmd_c4fpga.c / c4fpgaldr.c, in
 * u-boot-1.2.0.tgz+patches.tar.gz). The pin assignment comes from their
 * board-hammer.c: m2=GIO55 m0=GIO57 din=GIO58 cclk=GIO96 done=GIO97
 * prog=GIO98 initb=GIO7.
 *
 * INIT_B IS GIO7, WHICH IS ALSO THE FOUR UARTS' SHARED INTERRUPT. It is a
 * configuration status pin while the bitstream goes in and the serial IRQ
 * afterwards, so this driver releases it before populating the children — a
 * loader that keeps hold of it leaves every FPGA UART without a usable IRQ.
 *
 * Loading is triggered from sysfs rather than at probe. A built-in driver's
 * initcall runs before the rootfs exists, so request_firmware() at probe would
 * simply fail; more usefully, an explicit trigger means the bring-up loop is
 * "write the file, read done" instead of a fifteen-minute image rebuild.
 */

#include <linux/delay.h>
#include <linux/firmware.h>
#include <linux/gpio/consumer.h>
#include <linux/module.h>
#include <linux/of_platform.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/io.h>
#include <linux/sysfs.h>

#define IOX_FPGA_FW	"c4/iox-fpga.bin"

/* Direct GPIO register bit-bang (matches the vendor c4fpga). */
#define GPIO_PHYS	0x01c67000
#define GPIO_SIZE	0x100
#define DIN_SET		0x40
#define DIN_CLR		0x44
#define DIN_BIT		(1u << 26)	/* GIO58, bank1 */
#define CCLK_SET	0x90
#define CCLK_CLR	0x94
#define CCLK_BIT	(1u << 0)	/* GIO96, bank3 */


struct iox_fpga {
	struct device		*dev;
	struct gpio_desc	*m2, *m0, *din, *cclk, *prog;	/* out */
	struct gpio_desc	*done, *initb;			/* in  */
	bool			loaded;
	bool			populated;
	void __iomem		*gpio;	/* GPIO controller, for raw CCLK/DIN bit-bang */
};

/* ---- the wire protocol ------------------------------------------------- */

/*
 * One byte, MSB first. Data is presented on DIN and the FPGA samples it on the
 * RISING edge of CCLK, so the order is set-data, clock-low, clock-high.
 *
 * gpiod_set_value (not _cansleep) on purpose: these are memory-mapped SoC GPIOs
 * that never sleep, and a bitstream is a few million of these. Even so this is
 * the slow part — measured in seconds, not milliseconds — which is fine for a
 * once-per-boot operation and is why it is not in an atomic context.
 */
static void iox_fpga_byte(struct iox_fpga *f, u8 b)
{
	int i;

	for (i = 7; i >= 0; i--) {
		/* DIN = bit, MSB first, via the raw SET/CLR registers. */
		if ((b >> i) & 1)
			writel(DIN_BIT, f->gpio + DIN_SET);
		else
			writel(DIN_BIT, f->gpio + DIN_CLR);
		/* CCLK low then high; the FPGA samples DIN on the rising edge. */
		writel(CCLK_BIT, f->gpio + CCLK_CLR);
		writel(CCLK_BIT, f->gpio + CCLK_SET);
	}
}

static void iox_fpga_clocks(struct iox_fpga *f, unsigned int n)
{
	writel(DIN_BIT, f->gpio + DIN_SET);
	while (n--) {
		writel(CCLK_BIT, f->gpio + CCLK_CLR);
		writel(CCLK_BIT, f->gpio + CCLK_SET);
	}
}

static int iox_fpga_program(struct iox_fpga *f, const u8 *data, size_t len)
{
	unsigned int settle;
	size_t i;

	/* Mode pins select slave-serial. Held for the whole configuration. */
	gpiod_set_value(f->m2, 1);
	gpiod_set_value(f->m0, 1);
	gpiod_set_value(f->cclk, 0);

	/*
	 * INIT_B (GIO7) does NOT read back reliably on this board — gpiod, raw
	 * devmem and the gpio chardev all disagree, because the pin is shared
	 * with the FPGA UART interrupt. So it is logged for interest only and
	 * never gated on; DONE (GIO97), which reads cleanly, is the real signal.
	 */
	dev_info(f->dev, "start: done=%d (initb unreliable on this board)\n",
		 gpiod_get_value(f->done));

	/*
	 * Pulse PROG_B to clear the configuration memory. The descriptor is
	 * active-low in the device tree, so a logical 1 here drives the pin low
	 * — assert, hold, release.
	 */
	gpiod_set_value(f->prog, 0);
	udelay(20);
	gpiod_set_value(f->prog, 1);

	/*
	 * Hold PROG_B asserted long enough to clear the configuration memory,
	 * then release it. We do NOT wait on INIT_B here: on this board GIO7 is
	 * shared between INIT_B and the FPGA UART interrupt and does not read
	 * back the INIT_B line usefully (it reads 0 blank, and read 0 on the
	 * vendor even with the part configured). So a fixed settle replaces the
	 * INIT_B handshake — the same thing the vendor loader must do — and DONE
	 * is the signal we actually trust, below.
	 */
	udelay(500);
	gpiod_set_value(f->prog, 0);
	usleep_range(1000, 2000);	/* config-memory clear + INIT_B settle */

	for (i = 0; i < len; i++) {
		iox_fpga_byte(f, data[i]);
		/*
		 * No mid-stream INIT_B CRC check: INIT_B is not readable on this
		 * board (see above). A CRC failure shows up as DONE never rising,
		 * which we report below.
		 */
		if ((i & 0x3fff) == 0x3fff)
			cond_resched();
	}

	/*
	 * The start-up sequence needs clocks after the last data bit. The
	 * vendor's loader sends twelve; the Xilinx datasheets ask for rather
	 * more on some families, so this sends a generous number and then
	 * keeps clocking while it waits for DONE.
	 */
	iox_fpga_clocks(f, 64);
	for (settle = 0; settle < 200; settle++) {
		if (gpiod_get_value(f->done) > 0)
			break;
		iox_fpga_clocks(f, 8);
	}

	/*
	 * Judge success by the FPGA VERSION REGISTER, not the DONE pin. DONE and
	 * INIT_B read inconsistently on this board, but a configured FPGA drives
	 * its register bus and base+0 (== the node's reg[0], 0x04000200) reads a
	 * small version number (0x0004 on our bitstream); a blank part floats the
	 * bus to a repeating 0x02.. pattern.
	 */
	{
		struct resource *r = platform_get_resource(
			to_platform_device(f->dev), IORESOURCE_MEM, 0);
		void __iomem *base = r ? ioremap(r->start, resource_size(r)) : NULL;
		u16 ver = base ? readw(base) : 0xffff;

		if (base)
			iounmap(base);
		dev_info(f->dev, "after load: version reg = 0x%04x, DONE pin = %d\n",
			 ver, gpiod_get_value(f->done));
		if (ver == 0x0000 || ver == 0xffff || (ver & 0xff) == 0x02) {
			dev_err(f->dev,
				"FPGA did not configure (version 0x%04x is a floating bus) "
				"after %zu bytes\n", ver, len);
			return -EIO;
		}
		dev_info(f->dev, "FPGA CONFIGURED — version 0x%04x, %zu bytes\n",
			 ver, len);
	}
	return 0;
}

/*
 * Bring up what the FPGA now provides.
 *
 * Only after DONE, and only once. Before releasing INIT_B, because that pin
 * becomes the UARTs' shared interrupt the moment configuration ends and this
 * driver must not still own it when 8250_of asks for it.
 */
static int iox_fpga_populate(struct iox_fpga *f)
{
	int ret;

	if (f->populated)
		return 0;

	devm_gpiod_put(f->dev, f->initb);
	f->initb = NULL;

	ret = of_platform_populate(f->dev->of_node, NULL, NULL, f->dev);
	if (ret) {
		dev_err(f->dev, "the FPGA is configured but its children did not probe: %d\n", ret);
		return ret;
	}
	f->populated = true;
	dev_info(f->dev, "FPGA children populated — the UARTs and IR block are live\n");
	return 0;
}

static int iox_fpga_load(struct iox_fpga *f, const char *name)
{
	const struct firmware *fw;
	int ret;

	ret = request_firmware(&fw, name, f->dev);
	if (ret) {
		dev_err(f->dev, "no firmware %s: %d\n", name, ret);
		return ret;
	}
	dev_info(f->dev, "loading %s (%zu bytes)\n", name, fw->size);

	ret = iox_fpga_program(f, fw->data, fw->size);
	release_firmware(fw);
	if (ret)
		return ret;

	f->loaded = true;
	return iox_fpga_populate(f);
}

/* ---- sysfs ------------------------------------------------------------- */

static ssize_t firmware_store(struct device *dev, struct device_attribute *a,
			      const char *buf, size_t count)
{
	struct iox_fpga *f = dev_get_drvdata(dev);
	char name[128];
	int ret;

	if (count == 0 || count >= sizeof(name))
		return -EINVAL;
	memcpy(name, buf, count);
	name[count] = '\0';
	strim(name);

	ret = iox_fpga_load(f, name[0] ? name : IOX_FPGA_FW);
	return ret ? ret : count;
}
static DEVICE_ATTR_WO(firmware);

/*
 * The three pins worth reading from userspace during bring-up. DONE is the
 * answer to "did it work"; INIT_B distinguishes "not answering" from "bad
 * bitstream"; `loaded` says whether this driver believes it succeeded.
 */
static ssize_t done_show(struct device *dev, struct device_attribute *a, char *buf)
{
	struct iox_fpga *f = dev_get_drvdata(dev);

	return sysfs_emit(buf, "%d\n", gpiod_get_value(f->done));
}
static DEVICE_ATTR_RO(done);

static ssize_t initb_show(struct device *dev, struct device_attribute *a, char *buf)
{
	struct iox_fpga *f = dev_get_drvdata(dev);

	/* Released once the children are up; say so rather than oopsing. */
	if (!f->initb)
		return sysfs_emit(buf, "released\n");
	return sysfs_emit(buf, "%d\n", gpiod_get_value(f->initb));
}
static DEVICE_ATTR_RO(initb);

/*
 * Read the FPGA's register window through an UNCACHED mapping.
 *
 * This exists because userspace cannot answer the question. busybox devmem
 * maps /dev/mem, and that mapping is cached: writing a byte dirties a cache
 * line and reading it back hits that line, so every offset in the window looks
 * like perfectly good storage even with no FPGA driving the bus at all. The
 * giveaway was a fresh read of the same offsets returning a constant.
 *
 * ioremap gives a device mapping with no caching, so what comes back here is
 * what the bus returned. A window of identical bytes means nothing is driving
 * it; a 16550 that answers will show structure — LSR reading 0x60 when idle is
 * the easiest thing to recognise.
 */
static ssize_t window_show(struct device *dev, struct device_attribute *a, char *buf)
{
	struct iox_fpga *f = dev_get_drvdata(dev);
	struct resource *r;
	void __iomem *base;
	int len = 0, i;

	r = platform_get_resource(to_platform_device(dev), IORESOURCE_MEM, 0);
	if (!r)
		return sysfs_emit(buf, "no reg resource\n");

	base = ioremap(r->start, resource_size(r));
	if (!base)
		return sysfs_emit(buf, "ioremap failed\n");

	for (i = 0; i < 0x100; i += 16) {
		int j;

		len += sysfs_emit_at(buf, len, "%04x:", i);
		for (j = 0; j < 16; j++)
			len += sysfs_emit_at(buf, len, " %02x", readb(base + i + j));
		len += sysfs_emit_at(buf, len, "\n");
	}
	iounmap(base);
	return len;
}
static DEVICE_ATTR_RO(window);

static ssize_t loaded_show(struct device *dev, struct device_attribute *a, char *buf)
{
	struct iox_fpga *f = dev_get_drvdata(dev);

	return sysfs_emit(buf, "%d\n", f->loaded);
}
static DEVICE_ATTR_RO(loaded);

static struct attribute *iox_fpga_attrs[] = {
	&dev_attr_firmware.attr,
	&dev_attr_done.attr,
	&dev_attr_initb.attr,
	&dev_attr_loaded.attr,
	&dev_attr_window.attr,
	NULL,
};
ATTRIBUTE_GROUPS(iox_fpga);

/* ---- probe ------------------------------------------------------------- */

static int iox_fpga_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct iox_fpga *f;

	f = devm_kzalloc(dev, sizeof(*f), GFP_KERNEL);
	if (!f)
		return -ENOMEM;
	f->dev = dev;

	/*
	 * Outputs start deasserted: PROG_B released and CCLK low, so claiming
	 * the pins does not by itself disturb a part somebody already
	 * configured.
	 */
	f->m2    = devm_gpiod_get(dev, "m2",    GPIOD_OUT_LOW);
	f->m0    = devm_gpiod_get(dev, "m0",    GPIOD_OUT_LOW);
	f->din   = devm_gpiod_get(dev, "din",   GPIOD_OUT_LOW);
	f->cclk  = devm_gpiod_get(dev, "cclk",  GPIOD_OUT_LOW);
	f->prog  = devm_gpiod_get(dev, "prog",  GPIOD_OUT_LOW);
	f->done  = devm_gpiod_get(dev, "done",  GPIOD_IN);
	f->initb = devm_gpiod_get(dev, "initb", GPIOD_IN);

	if (IS_ERR(f->m2) || IS_ERR(f->m0) || IS_ERR(f->din) || IS_ERR(f->cclk) ||
	    IS_ERR(f->prog) || IS_ERR(f->done) || IS_ERR(f->initb)) {
		dev_err(dev, "missing a slave-serial GPIO — check the *-gpios properties\n");
		return -EINVAL;
	}

	f->gpio = devm_ioremap(dev, GPIO_PHYS, GPIO_SIZE);
	if (!f->gpio) {
		dev_err(dev, "cannot map the GPIO controller for bit-bang\n");
		return -ENOMEM;
	}

	platform_set_drvdata(pdev, f);

	/*
	 * If something already configured the part — vendor Linux loaded it and
	 * we reached openHC by a warm path that never power-cycled or pulsed
	 * PROG — the config SRAM is still live and the UARTs can come up now,
	 * with NO firmware write and, crucially, without pulsing PROG (which
	 * would wipe a working config). Detect it by the version register, not
	 * the DONE pin: DONE reads unreliably on this board, but a configured
	 * FPGA drives base+0 to a small version (0x0004), while a blank part
	 * floats the bus to a repeating 0x02.. pattern.
	 */
	{
		struct resource *r = platform_get_resource(pdev, IORESOURCE_MEM, 0);
		void __iomem *fb = r ? ioremap(r->start, resource_size(r)) : NULL;
		u16 ver = fb ? readw(fb) : 0xffff;
		bool configured = fb && ver != 0x0000 && ver != 0xffff &&
				  (ver & 0xff) != 0x02;

		if (fb)
			iounmap(fb);
		if (configured) {
			dev_info(dev, "FPGA already configured (version 0x%04x) — populating without a reload\n",
				 ver);
			f->loaded = true;
			iox_fpga_populate(f);
		} else {
			dev_info(dev, "FPGA is blank (version reg 0x%04x); write a filename to sysfs 'firmware' to load one\n",
				 ver);
		}
	}
	return 0;
}

static const struct of_device_id iox_fpga_of_match[] = {
	{ .compatible = "control4,iox-fpga" },
	{ }
};
MODULE_DEVICE_TABLE(of, iox_fpga_of_match);

static struct platform_driver iox_fpga_driver = {
	.driver = {
		.name		= "ohc-iox-fpga",
		.of_match_table	= iox_fpga_of_match,
		.dev_groups	= iox_fpga_groups,
	},
	.probe = iox_fpga_probe,
};
module_platform_driver(iox_fpga_driver);

MODULE_DESCRIPTION("Control4 IO Extender V1 FPGA slave-serial loader");
MODULE_LICENSE("GPL");
