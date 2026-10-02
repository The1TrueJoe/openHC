// SPDX-License-Identifier: GPL-2.0
/*
 * Control4 IO Extender V1 ("hammer") — FPGA IR output, as rc-core/lirc devices.
 *
 * Each rear IR jack is registered as its own lirc TX device named "openHC IR
 * out N" (1-based), exactly like the HC-800/EA IO-MCU path (gpio-ohc-iomcu.c) —
 * so iod's lirc.rs finds and drives them with the microsecond timings ir-ctl
 * speaks, with no board-specific API. Output only; this board has no IR RX.
 *
 * THE HARDWARE. Two identical IR engines ("accelerators") live in the FPGA at
 * 0x04000220 and 0x04000230. Either can drive any jack: which jacks it drives is
 * its output-enable mask. The layout comes from disassembling the vendor
 * c4irout.ko (c4irout_config/_setup/_go, whose debug strings name every field)
 * and was confirmed against a GC-IRL learner on jack 1. All registers are
 * 16-bit. Per engine, from its base:
 *   +0x00  OE        output enable, bit n = jack n+1 (all eight verified)
 *   +0x02  PERIOD    carrier period in 50 MHz clocks (Hz = 50e6 / PERIOD,
 *                    measured 50-100 kHz to the kHz)
 *   +0x04  CONTROL   bit15 GO, bit14 infinite, bit13 enable, bits 4..12 FIFO
 *                    watermark (vendor: 32), bit0 engine/FIFO reset
 *   +0x06  REPEAT    repeat count << 9
 *   +0x08  FIFO      durations in CARRIER PERIODS: mark = 0x8000 | n, space = n,
 *                    0x4000 | (n >> 14) high word first for n > 0x3fff, 0xc000 end
 *   +0x0a  RSTART    repeat start
 *   +0x0c  REND      repeat end - 1
 *   0x0400023e       inverted / no-carrier (shared by both engines)
 *
 * The FIFO holds a whole code (three back-to-back NEC frames, 204 words, came
 * out intact), so the train is loaded at kernel speed and then fired — no
 * watermark interrupt. One engine and a mutex is enough for one-jack-at-a-time
 * sends.
 * ponytail: engine 0 only; use engine 1 too if concurrent sends on two jacks matter.
 */

#include <linux/io.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/delay.h>
#include <linux/mutex.h>
#include <linux/math64.h>
#include <media/rc-core.h>

#define IR_EMITTERS	8
#define IR_CLK_HZ	50000000u
#define IR_DEFAULT_HZ	38000
#define IR_MAX_WORDS	512

/* Engine 0 registers, from the mapped base 0x04000220. */
#define IR_OE		0x00
#define IR_PERIOD	0x02
#define IR_CONTROL	0x04
#define IR_REPEAT	0x06
#define IR_FIFO		0x08
#define IR_RSTART	0x0a
#define IR_REND		0x0c
#define IR_INVERT	0x1e		/* 0x0400023e, shared */

#define CTRL_GO		0x8000
#define CTRL_ENABLE	0x2000
#define CTRL_WM32	(32 << 4)
#define CTRL_RESET	0x0001

#define FIFO_MARK	0x8000
#define FIFO_HIGH	0x4000
#define FIFO_END	0xc000
#define FIFO_DUR_MASK	0x3fff

struct iox_irout;

struct iox_emitter {
	struct iox_irout *ir;
	struct rc_dev	*rc;
	char		*name;
	u16		oe;		/* this jack's output-enable bit */
	u32		carrier;	/* Hz, from s_tx_carrier */
};

struct iox_irout {
	struct device		*dev;
	void __iomem		*base;
	struct mutex		lock;	/* one engine, one send at a time */
	struct iox_emitter	em[IR_EMITTERS];
	int			nem;
};

static void iox_ir_emit(struct iox_irout *ir, u16 oe, u32 hz, const u16 *dur, int n)
{
	void __iomem *b = ir->base;
	u32 period = DIV_ROUND_CLOSEST(IR_CLK_HZ, hz ? hz : IR_DEFAULT_HZ);
	int i;

	mutex_lock(&ir->lock);
	writew(oe, b + IR_OE);
	writew(clamp_val(period, 1, 0xffff), b + IR_PERIOD);
	writew(0, b + IR_REPEAT);
	writew(0, b + IR_RSTART);
	writew(0xffff, b + IR_REND);
	writew(0, b + IR_INVERT);
	writew(CTRL_ENABLE | CTRL_WM32, b + IR_CONTROL);
	writew(CTRL_ENABLE | CTRL_WM32 | CTRL_RESET, b + IR_CONTROL);
	writew(CTRL_ENABLE | CTRL_WM32, b + IR_CONTROL);

	for (i = 0; i < n; i++) {
		u16 d = dur[i];
		bool mark = !(i & 1);		/* even = mark (pulse), odd = space */

		if (d > FIFO_DUR_MASK)
			writew(FIFO_HIGH | ((d >> 14) & FIFO_DUR_MASK), b + IR_FIFO);
		writew((mark ? FIFO_MARK : 0) | (d & FIFO_DUR_MASK), b + IR_FIFO);
	}
	writew(FIFO_END, b + IR_FIFO);
	writew(CTRL_ENABLE | CTRL_WM32 | CTRL_GO, b + IR_CONTROL);

	/* GO clears when the train is out. A full NEC frame is ~108 ms. */
	for (i = 0; i < 2000 && (readw(b + IR_CONTROL) & CTRL_GO); i++)
		usleep_range(500, 1000);
	if (readw(b + IR_CONTROL) & CTRL_GO)
		dev_warn(ir->dev, "IR engine still busy after send (CONTROL %#06x)\n",
			 readw(b + IR_CONTROL));
	writew(0, b + IR_OE);			/* no jack driven between sends */
	mutex_unlock(&ir->lock);
}

/* ---- lirc TX (what iod drives) ----------------------------------------- */

static int iox_tx_ir(struct rc_dev *rcdev, unsigned int *txbuf, unsigned int count)
{
	struct iox_emitter *em = rcdev->priv;
	u32 hz = em->carrier ? em->carrier : IR_DEFAULT_HZ;
	unsigned int i, n = min_t(unsigned int, count, IR_MAX_WORDS);
	u16 *dur;

	if (!n)
		return 0;
	dur = kmalloc_array(n, sizeof(*dur), GFP_KERNEL);
	if (!dur)
		return -ENOMEM;
	/* rc-core hands us microseconds (mark first); the FIFO wants carrier
	 * periods. div_u64 because the kernel links no libgcc. */
	for (i = 0; i < n; i++)
		dur[i] = min_t(u64, div_u64((u64)txbuf[i] * hz + 500000, 1000000u), 0xffff);
	iox_ir_emit(em->ir, em->oe, hz, dur, n);
	kfree(dur);
	return n;
}

static int iox_tx_carrier(struct rc_dev *rcdev, u32 carrier)
{
	struct iox_emitter *em = rcdev->priv;

	/* The period register is 16 bits of 50 MHz clocks: 763 Hz is the floor. */
	if (carrier < 1000 || carrier > 500000)
		return -EINVAL;
	em->carrier = carrier;
	return 0;
}

/* ---- probe ------------------------------------------------------------- */

static int iox_register_emitter(struct iox_irout *ir, int i)
{
	struct iox_emitter *em = &ir->em[i];
	struct rc_dev *rc;
	int ret;

	em->ir = ir;
	em->carrier = IR_DEFAULT_HZ;
	/* Bit n = jack n+1, verified on all eight against a learner: each
	 * send was heard on its own jack and no other. */
	em->oe = BIT(i);
	em->name = kasprintf(GFP_KERNEL, "openHC IR out %d", i + 1);
	if (!em->name)
		return -ENOMEM;

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
	mutex_init(&ir->lock);

	r = platform_get_resource(pdev, IORESOURCE_MEM, 0);	/* 0x04000220 */
	if (!r)
		return -EINVAL;
	ir->base = devm_ioremap(&pdev->dev, r->start, 0x20);	/* both engines + 0x3e */
	if (!ir->base)
		return -ENOMEM;
	writew(0, ir->base + IR_OE);
	writew(0, ir->base + 0x10 + IR_OE);	/* engine 1 stays off */
	platform_set_drvdata(pdev, ir);

	for (i = 0; i < IR_EMITTERS; i++)
		if (iox_register_emitter(ir, i) == 0)
			ir->nem++;

	dev_info(&pdev->dev, "IR out: %d lirc emitters\n", ir->nem);
	return ir->nem ? 0 : -ENODEV;
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
	.probe = iox_irout_probe,
};
module_platform_driver(iox_irout_driver);

MODULE_DESCRIPTION("Control4 IO Extender V1 FPGA IR output");
MODULE_LICENSE("GPL");
