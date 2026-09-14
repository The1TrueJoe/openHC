// SPDX-License-Identifier: GPL-2.0
/*
 * Control4 IO Extender V1 ("hammer") — FPGA IR output, as rc-core/lirc devices.
 *
 * Each rear IR jack is registered as its own lirc TX device named "openHC IR
 * out N" (1-based), exactly like the HC-800/EA IO-MCU path (gpio-ohc-iomcu.c) —
 * so iod's lirc.rs finds and drives them with the microsecond timings ir-ctl
 * speaks, with no board-specific API. Output only; this board has no IR RX.
 *
 * THE HARDWARE. Eight emitters live in two register blocks inside the FPGA at
 * EMIF offsets 0x20 and 0x30 from the FPGA base (physical 0x04000220/0x230).
 * Recovered by disassembling the vendor c4irout.ko + c4fpga.ko (Control4 never
 * shipped c4irout.c) and confirmed on hardware. All registers are 16-bit (the
 * whole FPGA is 8/16-bit in the low half of each EMIF word; see the UART
 * reg-shift-1 note). Per block, offsets from the block base:
 *   +0x02  timing    (vendor writes 0x017e)
 *   +0x04  CONTROL   bit15 GO, bit14 MODE (fpga-handled), bit13 ENABLE,
 *                    bits 4..12 select (which output jack)
 *   +0x06  CARRIER   = (v & 0x7f) << 9   (a prescaler; v→Hz calibrated live)
 *   +0x08  DATA FIFO write-only pulse/space stream
 *   +0x0c  COUNT     (vendor writes config-1; 0xffff default)
 *   0x3e (global)    (vendor writes 0)
 *
 * FIFO ENCODING (from c4irout_write): each duration, in CARRIER PERIODS, is one
 * word — mark (carrier on) = 0x8000 | (periods & 0x3fff), space = 0x4000 |
 * (periods & 0x3fff). The train ends with 0xc000. Fire by setting GO in CONTROL.
 * The FIFO must be filled at kernel speed: a slow userspace poke underruns it
 * and nothing clean comes out, which is the whole reason this is a driver.
 *
 * BRING-UP KNOBS. The carrier prescaler→Hz curve and the select→jack map are the
 * two values that need pinning against an IR learner. Until they are, sysfs
 * exposes cal_carrier/cal_block/cal_select and a `send` that takes a raw
 * pulse/space list in carrier periods, so the emit path can be exercised and
 * measured directly; the lirc tx_ir path below uses the same emit primitive.
 */

#include <linux/io.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/delay.h>
#include <linux/sysfs.h>
#include <linux/math64.h>
#include <media/rc-core.h>

#define IR_EMITTERS	8

/* Block bases within the mapped region (which starts at 0x04000220). */
#define BLK0		0x00
#define BLK1		0x10
#define GLOBAL		0x1e		/* 0x0400023e, shared */

/* Register offsets within a block. */
#define IR_AUX0		0x00
#define IR_TIMING	0x02
#define IR_CONTROL	0x04
#define IR_CARRIER	0x06
#define IR_FIFO		0x08
#define IR_AUX_A	0x0a
#define IR_COUNT	0x0c

#define CTRL_GO		0x8000
#define CTRL_MODE	0x4000
#define CTRL_ENABLE	0x2000
#define CTRL_BUSY	0x0008
#define CTRL_SEL_MASK	0x1ff0

/*
 * FIFO word encoding (from c4irout_write, corrected):
 *   mark  low word  = 0x8000 | (d & 0x3fff)   (carrier ON, bit15 = mark)
 *   space low word  =          (d & 0x3fff)   (carrier OFF, no flag)
 *   high word       = 0x4000 | ((d >> 14) & 0x3fff)   (only if d > 0x3fff;
 *                     upper bits, emitted BEFORE the low word, mark or space)
 *   terminator      = 0xc000
 * 0x4000 is the extended-duration marker, NOT a space flag — that was the bug.
 */
#define FIFO_MARK	0x8000
#define FIFO_HIGH	0x4000
#define FIFO_END	0xc000
#define FIFO_DUR_MASK	0x3fff

#define IR_MAX_WORDS	512
#define IR_DEFAULT_HZ	38000

struct iox_emitter {
	struct iox_irout *ir;
	struct rc_dev	*rc;
	char		*name;
	u32		block;		/* 0 or 1 */
	u32		select;		/* CONTROL select field */
	u32		carrier;	/* Hz, from s_tx_carrier */
};

struct iox_irout {
	struct device		*dev;
	void __iomem		*base;	/* mapped at 0x04000220, both blocks */
	struct iox_emitter	em[IR_EMITTERS];
	int			nem;
	/* bring-up calibration knobs (see sysfs) */
	u32			cal_carrier;
	u32			cal_block;
	u32			cal_select;
};

/*
 * Carrier register from a frequency. HYPOTHESIS (UNVERIFIED — calibrate against
 * an IR learner or a scope): the FPGA clocks the carrier off ~50 MHz and the
 * CARRIER reg = (v & 0x7f) << 9 is a /512 prescaler, so v ≈ 50e6/(512*Hz) =
 * IR_CARRIER_NUM/Hz. If a measurement disagrees this one constant is the fix.
 * v is clamped to 1..64 (the vendor ioctl caps config[0x38] at 64). Overridable
 * live via the cal_carrier sysfs knob (raw reg value; 0 = use this formula).
 */
#define IR_CARRIER_NUM	97656u
static u16 iox_carrier_reg(u32 hz)
{
	u32 v = hz ? (IR_CARRIER_NUM + hz / 2) / hz : 3;

	if (v < 1)
		v = 1;
	else if (v > 64)
		v = 64;
	return (u16)((v & 0x7f) << 9);
}

/*
 * The one emit primitive. `dur` are durations already in CARRIER PERIODS,
 * alternating mark/space with a mark first. Fills the FIFO at kernel speed and
 * fires GO on the given block with the given select and raw carrier register.
 */
static void iox_ir_emit(struct iox_irout *ir, u32 block, u32 select,
			u32 carrier_reg, const u16 *dur, int n)
{
	void __iomem *b = ir->base + (block ? BLK1 : BLK0);
	/* CONTROL = ENABLE | select. NO mode bit (0x4000): the vendor's own emit
	 * leaves CONTROL at 0x2200 (enable + select 0x200), and setting the mode
	 * bit wedges the block busy. Confirmed by watching a real /dev/irout0
	 * write on the stock OS. */
	u16 ctrl = CTRL_ENABLE | (select & CTRL_SEL_MASK);
	int i;

	writew(0, b + IR_CONTROL);
	writew(0x017e, b + IR_TIMING);
	writew(0, b + IR_AUX0);
	writew(ctrl, b + IR_CONTROL);
	writew(carrier_reg & 0xffff, b + IR_CARRIER);
	writew(0, ir->base + GLOBAL);
	writew(0, b + IR_AUX_A);
	writew(0xffff, b + IR_COUNT);

	for (i = 0; i < n; i++) {
		u16 d = dur[i];
		bool mark = !(i & 1);		/* even = mark (pulse), odd = space */

		if (d > FIFO_DUR_MASK)		/* long duration: high word first */
			writew(FIFO_HIGH | ((d >> 14) & FIFO_DUR_MASK), b + IR_FIFO);
		writew((mark ? FIFO_MARK : 0) | (d & FIFO_DUR_MASK), b + IR_FIFO);
	}
	writew(FIFO_END, b + IR_FIFO);
	writew(ctrl | CTRL_GO, b + IR_CONTROL);

	for (i = 0; i < 4000; i++) {		/* best-effort wait for done */
		if (!(readw(b + IR_CONTROL) & CTRL_BUSY))
			break;
		udelay(50);
	}
	dev_info(ir->dev, "emit blk%u sel%#x carrier=%#06x words=%d -> CONTROL=%#06x\n",
		 block, select, carrier_reg, n, readw(b + IR_CONTROL));
}

/* ---- lirc TX (what iod drives) ----------------------------------------- */

static int iox_tx_ir(struct rc_dev *rcdev, unsigned int *txbuf, unsigned int count)
{
	struct iox_emitter *em = rcdev->priv;
	struct iox_irout *ir = em->ir;
	u32 hz = em->carrier ? em->carrier : IR_DEFAULT_HZ;
	/* cal_carrier (sysfs) overrides when set; else derive the reg from Hz. */
	u32 carrier_reg = ir->cal_carrier ? ir->cal_carrier : iox_carrier_reg(hz);
	u16 *dur;
	unsigned int i, n;

	if (!count)
		return 0;
	n = min_t(unsigned int, count, IR_MAX_WORDS);
	dur = kmalloc_array(n, sizeof(*dur), GFP_KERNEL);
	if (!dur)
		return -ENOMEM;

	/* rc-core hands us microseconds (mark first). Convert to carrier periods:
	 * periods = us * Hz / 1e6. div_u64 because the kernel links no libgcc. */
	for (i = 0; i < n; i++) {
		u32 p = div_u64((u64)txbuf[i] * hz, 1000000u);
		/* u16 range; iox_ir_emit splits >0x3fff into a high word. */
		dur[i] = p > 0xffff ? 0xffff : p;
	}
	iox_ir_emit(ir, em->block, em->select, carrier_reg, dur, n);
	kfree(dur);
	return n;
}

static int iox_tx_carrier(struct rc_dev *rcdev, u32 carrier)
{
	struct iox_emitter *em = rcdev->priv;

	if (carrier < 1000)
		return -EINVAL;
	em->carrier = carrier;
	return 0;
}

/* ---- sysfs bring-up / calibration harness ------------------------------ */

#define CAL_ATTR(field)							\
static ssize_t field##_show(struct device *d, struct device_attribute *a,\
			    char *buf)					\
{									\
	struct iox_irout *ir = dev_get_drvdata(d);			\
	return sysfs_emit(buf, "%#x\n", ir->field);			\
}									\
static ssize_t field##_store(struct device *d, struct device_attribute *a,\
			     const char *buf, size_t n)			\
{									\
	struct iox_irout *ir = dev_get_drvdata(d);			\
	u32 v;								\
	if (kstrtou32(buf, 0, &v))					\
		return -EINVAL;						\
	ir->field = v;							\
	return n;							\
}									\
static DEVICE_ATTR_RW(field)

CAL_ATTR(cal_carrier);
CAL_ATTR(cal_block);
CAL_ATTR(cal_select);

/* Raw pulse/space list in carrier periods (mark first) on cal_block/select. */
static ssize_t send_store(struct device *d, struct device_attribute *a,
			  const char *buf, size_t n)
{
	struct iox_irout *ir = dev_get_drvdata(d);
	u16 *dur;
	int cnt = 0;
	const char *p = buf;

	dur = kmalloc_array(IR_MAX_WORDS, sizeof(*dur), GFP_KERNEL);
	if (!dur)
		return -ENOMEM;
	while (*p && cnt < IR_MAX_WORDS) {
		unsigned int v;
		int used;
		while (*p == ' ' || *p == '\t' || *p == '\n' || *p == ',')
			p++;
		if (!*p || sscanf(p, "%u%n", &v, &used) != 1)
			break;
		dur[cnt++] = (u16)v;
		p += used;
	}
	if (cnt)
		iox_ir_emit(ir, ir->cal_block, ir->cal_select,
			    ir->cal_carrier ? ir->cal_carrier
					    : iox_carrier_reg(IR_DEFAULT_HZ),
			    dur, cnt);
	kfree(dur);
	return n;
}
static DEVICE_ATTR_WO(send);

static ssize_t regs_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct iox_irout *ir = dev_get_drvdata(d);
	int len = 0, o;

	for (o = 0; o < 0x20; o += 2)
		len += sysfs_emit_at(buf, len, "%04x=%04x%c", 0x220 + o,
				     readw(ir->base + o), (o % 16 == 14) ? '\n' : ' ');
	return len;
}
static DEVICE_ATTR_RO(regs);

static struct attribute *iox_irout_attrs[] = {
	&dev_attr_cal_carrier.attr,
	&dev_attr_cal_block.attr,
	&dev_attr_cal_select.attr,
	&dev_attr_send.attr,
	&dev_attr_regs.attr,
	NULL,
};
ATTRIBUTE_GROUPS(iox_irout);

/* ---- probe ------------------------------------------------------------- */

static int iox_register_emitter(struct iox_irout *ir, int i)
{
	struct iox_emitter *em = &ir->em[i];
	struct rc_dev *rc;
	int ret;

	em->ir = ir;
	em->carrier = IR_DEFAULT_HZ;
	/* PROVISIONAL map (block/select per jack) — corrected once the live
	 * select→jack calibration is in; the cal_* sysfs path is what pins it. */
	em->block = (i < 4) ? 0 : 1;
	em->select = 0x200;

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

	r = platform_get_resource(pdev, IORESOURCE_MEM, 0);	/* 0x04000220 */
	if (!r)
		return -EINVAL;
	ir->base = devm_ioremap(&pdev->dev, r->start, 0x20);	/* both blocks + 0x3e */
	if (!ir->base)
		return -ENOMEM;

	ir->cal_carrier = 0;		/* 0 = derive carrier reg from Hz (see iox_carrier_reg) */
	ir->cal_block = 0;
	ir->cal_select = 0x200;
	platform_set_drvdata(pdev, ir);

	for (i = 0; i < IR_EMITTERS; i++)
		if (iox_register_emitter(ir, i) == 0)
			ir->nem++;

	dev_info(&pdev->dev,
		 "IR out: %d lirc emitters; sysfs cal_carrier/cal_block/cal_select/send/regs\n",
		 ir->nem);
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
		.dev_groups	= iox_irout_groups,
	},
	.probe = iox_irout_probe,
};
module_platform_driver(iox_irout_driver);

MODULE_DESCRIPTION("Control4 IO Extender V1 FPGA IR output");
MODULE_LICENSE("GPL");
