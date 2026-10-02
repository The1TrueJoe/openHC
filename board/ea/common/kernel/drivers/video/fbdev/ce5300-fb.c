// SPDX-License-Identifier: GPL-2.0
/*
 * Intel CE5300 (Atom CE5310) inherited-framebuffer glue.
 *
 * CEFDK leaves the VDC display pipe enabled with a 720x480 timing and the
 * upp_c/upp_e planes scanning ARGB8888 out of a fixed physical buffer. None of
 * that needs a modeset driver to keep working -- it survives into Linux
 * untouched. So all we do is hand that buffer to simple-framebuffer.
 *
 * Values recovered from the vendor gdl_server.ko register map and verified on
 * an EA-3 (board v2):
 *   pipe A enable   BAR0(01:08.0, 0xdfb00000) + 0x70008 bit31   = set by firmware
 *   upp_c ctrl      + 0x31168 = 0x08FF4C01   (clock on + plane enable)
 *   upp_c surface   + 0x31100 = 0x7FC00000
 *   upp_c stride    + 0x31118 = 0x0B80       (2944 B = 736 px * 4)
 *
 * ponytail: the address and geometry are module params rather than probed off
 * BAR0, because probing means a PCI driver and an ioremap for four reads that
 * firmware has never once varied. Override on the command line if a board turns
 * up that does; make it a real PCI driver only if one actually does.
 */

#include <linux/init.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/mm.h>
#include <linux/ioport.h>
#include <linux/pfn.h>
#include <linux/platform_data/simplefb.h>

static unsigned long fb_base = 0x7FC00000;
module_param(fb_base, ulong, 0444);
MODULE_PARM_DESC(fb_base, "scanout buffer physical address");

static unsigned int fb_width = 720, fb_height = 480, fb_stride = 2944;
module_param(fb_width, uint, 0444);
module_param(fb_height, uint, 0444);
module_param(fb_stride, uint, 0444);

static struct simplefb_platform_data ce5300_fb_pdata;
static struct platform_device *ce5300_fb_dev;

static int __init ce5300_fb_init(void)
{
	struct resource res;

	ce5300_fb_pdata.width  = fb_width;
	ce5300_fb_pdata.height = fb_height;
	ce5300_fb_pdata.stride = fb_stride;
	ce5300_fb_pdata.format = "a8r8g8b8";

	/* The buffer sits in an e820-reserved window, not System RAM, so it is
	 * ours to map -- but refuse anything that would land in kernel memory.
	 */
	if (page_is_ram(PFN_DOWN(fb_base))) {
		pr_err("ce5300-fb: 0x%lx is System RAM, refusing\n", fb_base);
		return -EINVAL;
	}

	memset(&res, 0, sizeof(res));
	res.flags = IORESOURCE_MEM;
	res.name  = "ce5300-fb";
	res.start = fb_base;
	res.end   = fb_base + (resource_size_t)fb_stride * fb_height - 1;

	ce5300_fb_dev = platform_device_register_resndata(NULL,
			"simple-framebuffer", 0, &res, 1,
			&ce5300_fb_pdata, sizeof(ce5300_fb_pdata));

	return PTR_ERR_OR_ZERO(ce5300_fb_dev);
}

static void __exit ce5300_fb_exit(void)
{
	platform_device_unregister(ce5300_fb_dev);
}

module_init(ce5300_fb_init);
module_exit(ce5300_fb_exit);

MODULE_DESCRIPTION("Intel CE5300 inherited firmware framebuffer");
MODULE_LICENSE("GPL");
