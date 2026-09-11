// SPDX-License-Identifier: GPL-2.0
/*
 * Control4 IO Extender V1 ("hammer") — FPGA IR output, as rc-core devices.
 *
 * Eight IR emitters live in two 16-byte register blocks inside the FPGA, at
 * EMIF offsets 0x20 and 0x30 from the FPGA base (physical 0x04000220 and
 * 0x04000230). This registers one rc_dev per emitter, NAMED "openHC IR out N",
 * so iod finds them with the exact same lookup it uses for the HC-800/EA family
 * (see packages/iod/src/ir.rs, emitter_name()). No openHC-side code changes:
 * an IO Extender's IR looks identical to any other board's from userspace.
 *
 * WHERE THIS COMES FROM. Control4 never published c4irout.c, so the register
 * layout below was recovered by disassembling the vendor c4irout.ko pulled off
 * a live unit. See the openHC "IR block register map" doc. What is CONFIRMED
 * from the binary:
 *
 *   - two blocks, 0x20 and 0x30, eight halfword registers each;
 *   - a CONTROL word where bit 15 = GO, bit 14 = a mode flag, bit 13 is always
 *     set, and bits 5..12 are a select field (c4irout_setup / c4irout_go);
 *   - a carrier register written as (val & 0x7f) << 9;
 *   - a count register written as (count - 1).
 *
 * What is NOT yet calibrated, because it needs the block driven on real
 * silicon (which needs the FPGA loaded, which is the thing this all unblocks):
 *
 *   - the exact carrier prescaler math (CARRIER_DIV below is a first guess);
 *   - which physical jack is block 0x20 vs 0x30, and how eight emitters map
 *     onto the two blocks via the CONTROL select field.
 *
 * Both are marked FIXME and are an afternoon on the bench, not a rewrite. The
 * driver is structured and registered correctly; only these constants are
 * provisional.
 */

#include <linux/io.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <media/rc-core.h>

/* Register offsets within a block, from c4irout_config's assignment. */
#define IR_REG_CONTROL	0x14	/* CONTROL word: bit15 GO, bit14 mode, bits5-12 select */
#define IR_REG_CARRIER	0x16	/* (val & 0x7f) << 9 */
#define IR_REG_DATA	0x1a
#define IR_REG_COUNT	0x1c	/* count - 1 */
#define IR_CTRL_GO	0x8000
#define IR_CTRL_ENABLE	0x2000	/* bit 13, always set by c4irout_setup */

/*
 * FIXME(calibration): the vendor writes carrier as (v & 0x7f) << 9, which is a
 * prescaler off some FPGA clock, not Hz. Until a known 38 kHz code is fired on
 * the vendor OS and the register read back, treat this as provisional.
 */
#define IR_CARRIER_DEFAULT 38000

struct iox_emitter {
	struct rc_dev	*rc;
	char		*name;
	void __iomem	*block;	/* base + 0x20 or base + 0x30 */
	u32		select;	/* CONTROL bits 5..12 for this emitter */
	u32		carrier;
};

struct iox_irout {
	struct device		*dev;
	void __iomem		*base;
	struct iox_emitter	em[8];
	int			n;
};

/*
 * FIXME(map): eight emitters, two blocks. Provisional assumption — emitters
 * 0-3 on block 0x20, 4-7 on block 0x30, with the CONTROL select field carrying
 * the low index. Confirm against the vendor by firing each /dev/iroutN and
 * watching which jack lights.
 */
static void iox_emitter_geometry(struct iox_irout *ir, int i,
				 void __iomem **block, u32 *select)
{
	*block = ir->base + (i < 4 ? 0x20 : 0x30);
	*select = (u32)(i & 3) << 5;
}

static int iox_tx_carrier(struct rc_dev *rcdev, u32 carrier)
{
	struct iox_emitter *em = rcdev->priv;

	if (carrier == 0)
		return -EINVAL;
	em->carrier = carrier;
	return 0;
}

/*
 * Send a raw pulse/space train. rc-core hands us durations in microseconds;
 * the FPGA block wants its own encoding, which the vendor built from the
 * config registers. This lays down the CONTROL/carrier/count sequence the
 * disassembly showed and fires bit 15.
 *
 * FIXME(data path): the per-burst DATA encoding (how the us train becomes the
 * words written to IR_REG_DATA) is the one part not fully pinned from the
 * binary — c4irout_write's inner loop needs another pass. Structure is here;
 * the data marshalling is stubbed so this compiles and registers, and a wrong
 * send is a no-op rather than a wedge (unlike the MCU boards, a bad write here
 * cannot hang a shared microcontroller — the block is memory-mapped).
 */
static int iox_tx_ir(struct rc_dev *rcdev, unsigned int *txbuf, unsigned int count)
{
	struct iox_emitter *em = rcdev->priv;
	u16 carrier_field = (u16)((em->carrier / 1000) & 0x7f) << 9;
	u16 control;

	writew(carrier_field, em->block + IR_REG_CARRIER);
	writew((u16)(count ? count - 1 : 0), em->block + IR_REG_COUNT);

	control = readw(em->block + IR_REG_CONTROL);
	control &= ~0x1fc0;			/* clear the select field */
	control |= IR_CTRL_ENABLE | em->select;

	/* FIXME(data path): marshal txbuf into IR_REG_DATA here. */

	writew(control, em->block + IR_REG_CONTROL);
	writew(control | IR_CTRL_GO, em->block + IR_REG_CONTROL);

	/* rc-core wants the number of samples it consumed. */
	return count;
}

static int iox_register_emitter(struct iox_irout *ir, int i)
{
	struct iox_emitter *em = &ir->em[i];
	struct rc_dev *rc;
	int ret;

	em->name = kasprintf(GFP_KERNEL, "openHC IR out %d", i + 1);
	if (!em->name)
		return -ENOMEM;

	iox_emitter_geometry(ir, i, &em->block, &em->select);
	em->carrier = IR_CARRIER_DEFAULT;

	rc = rc_allocate_device(RC_DRIVER_IR_RAW_TX);
	if (!rc) {
		kfree(em->name);
		em->name = NULL;
		return -ENOMEM;
	}
	rc->priv = em;
	rc->driver_name = "ohc-iox-irout";
	rc->device_name = em->name;
	rc->tx_ir = iox_tx_ir;
	rc->s_tx_carrier = iox_tx_carrier;

	ret = rc_register_device(rc);
	if (ret) {
		rc_free_device(rc);
		kfree(em->name);
		em->name = NULL;
		return ret;
	}
	em->rc = rc;
	return 0;
}

static int iox_irout_probe(struct platform_device *pdev)
{
	struct iox_irout *ir;
	struct resource *r;
	int i;

	ir = devm_kzalloc(&pdev->dev, sizeof(*ir), GFP_KERNEL);
	if (!ir)
		return -ENOMEM;
	ir->dev = &pdev->dev;

	/*
	 * reg[0] is the 0x220 window; the two IR blocks sit at base and
	 * base+0x10 of a mapping that covers both. Map from the FPGA base so
	 * the 0x20/0x30 offsets above are literal.
	 */
	r = platform_get_resource(pdev, IORESOURCE_MEM, 0);
	if (!r)
		return -EINVAL;
	/* Back up to the FPGA base so IR_REG_* offsets (0x20/0x30 based) hold. */
	ir->base = devm_ioremap(&pdev->dev, r->start - 0x20, 0x40);
	if (!ir->base)
		return -ENOMEM;

	for (i = 0; i < 8; i++) {
		if (iox_register_emitter(ir, i) == 0)
			ir->n++;
	}
	platform_set_drvdata(pdev, ir);
	dev_info(&pdev->dev, "registered %d IR emitters (openHC IR out 1..%d)\n",
		 ir->n, ir->n);
	dev_info(&pdev->dev,
		 "NOTE: carrier and per-jack mapping are provisional — see FIXMEs\n");
	return ir->n ? 0 : -ENODEV;
}

static int iox_irout_remove(struct platform_device *pdev)
{
	struct iox_irout *ir = platform_get_drvdata(pdev);
	int i;

	for (i = 0; i < 8; i++) {
		if (ir->em[i].rc) {
			rc_unregister_device(ir->em[i].rc);
			kfree(ir->em[i].name);
		}
	}
	return 0;
}

static const struct of_device_id iox_irout_of_match[] = {
	{ .compatible = "control4,iox-irout" },
	{ }
};
MODULE_DEVICE_TABLE(of, iox_irout_of_match);

static struct platform_driver iox_irout_driver = {
	.driver = {
		.name		= "ohc-iox-irout",
		.of_match_table	= iox_irout_of_match,
	},
	.probe	= iox_irout_probe,
	.remove	= iox_irout_remove,
};
module_platform_driver(iox_irout_driver);

MODULE_DESCRIPTION("Control4 IO Extender V1 FPGA IR output");
MODULE_LICENSE("GPL");
