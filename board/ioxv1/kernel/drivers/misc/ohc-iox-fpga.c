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

struct iox_fpga {
	struct device		*dev;
	struct gpio_desc	*m2, *m0, *din, *cclk, *prog;	/* out */
	struct gpio_desc	*done, *initb;			/* in  */
	bool			loaded;
	bool			populated;
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
		gpiod_set_value(f->din, (b >> i) & 1);
		gpiod_set_value(f->cclk, 0);
		gpiod_set_value(f->cclk, 1);
	}
}

static void iox_fpga_clocks(struct iox_fpga *f, unsigned int n)
{
	gpiod_set_value(f->din, 1);
	while (n--) {
		gpiod_set_value(f->cclk, 0);
		gpiod_set_value(f->cclk, 1);
	}
}

static int iox_fpga_program(struct iox_fpga *f, const u8 *data, size_t len)
{
	unsigned int settle;
	bool saw_initb_low;
	size_t i;

	/* Mode pins select slave-serial. Held for the whole configuration. */
	gpiod_set_value(f->m2, 1);
	gpiod_set_value(f->m0, 1);
	gpiod_set_value(f->cclk, 0);

	dev_info(f->dev, "before PROG_B: initb=%d done=%d\n",
		 gpiod_get_value(f->initb), gpiod_get_value(f->done));

	/*
	 * Pulse PROG_B to clear the configuration memory. The descriptor is
	 * active-low in the device tree, so a logical 1 here drives the pin low
	 * — assert, hold, release.
	 */
	gpiod_set_value(f->prog, 0);
	udelay(20);
	gpiod_set_value(f->prog, 1);

	/*
	 * THE diagnostic that matters. INIT_B is driven LOW by the part while it
	 * clears configuration memory, so seeing it drop is proof that PROG_B
	 * actually reached the FPGA. If it never drops, nothing downstream is
	 * worth believing — the bitstream is fine and the pin is not.
	 */
	saw_initb_low = false;
	for (settle = 0; settle < 200; settle++) {
		if (gpiod_get_value(f->initb) == 0) {
			saw_initb_low = true;
			break;
		}
		udelay(10);
	}
	dev_info(f->dev, "PROG_B asserted: INIT_B %s\n",
		 saw_initb_low ? "went LOW (the part saw it)"
			       : "STAYED HIGH — PROG_B is not reaching the part");

	udelay(500);
	gpiod_set_value(f->prog, 0);

	/*
	 * INIT_B rises when the part has finished clearing and is ready for
	 * data. Waiting for it rather than a fixed delay is what makes a
	 * too-fast host safe, and a timeout here is the single most useful
	 * failure to report: it means the part is not answering the mode/prog
	 * pins at all, i.e. a wiring or polarity problem rather than a bad
	 * bitstream.
	 */
	for (settle = 0; settle < 1000; settle++) {
		if (gpiod_get_value(f->initb) > 0)
			break;
		usleep_range(100, 200);
	}
	if (gpiod_get_value(f->initb) <= 0) {
		dev_err(f->dev, "INIT_B never rose after PROG_B — check the mode and prog pins\n");
		return -ETIMEDOUT;
	}

	for (i = 0; i < len; i++) {
		iox_fpga_byte(f, data[i]);
		/*
		 * INIT_B falling MID-STREAM is the part telling us the bitstream
		 * failed its CRC. Say so, rather than clocking in another two
		 * megabytes and reporting a bare "DONE never came".
		 */
		if ((i & 0xffff) == 0xffff && gpiod_get_value(f->initb) == 0) {
			dev_err(f->dev, "INIT_B fell at byte %zu — bitstream CRC error\n", i);
			return -EIO;
		}
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
	if (gpiod_get_value(f->done) <= 0) {
		dev_err(f->dev,
			"DONE never rose after %zu bytes (initb=%d, PROG_B %s)\n",
			len, gpiod_get_value(f->initb),
			saw_initb_low ? "was seen by the part"
				      : "was NOT seen — suspect that first");
		return -EIO;
	}

	dev_info(f->dev, "FPGA configured from %zu bytes, DONE is high\n", len);
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

	platform_set_drvdata(pdev, f);

	/*
	 * If something already configured the part (a warm reboot, or U-Boot on
	 * a unit whose environment does it), the children can come up now and
	 * nobody has to write to sysfs at all.
	 */
	if (gpiod_get_value(f->done) > 0) {
		dev_info(dev, "DONE is already high — the FPGA is configured\n");
		f->loaded = true;
		iox_fpga_populate(f);
	} else {
		dev_info(dev, "FPGA is blank; write a filename to sysfs 'firmware' to load one\n");
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
