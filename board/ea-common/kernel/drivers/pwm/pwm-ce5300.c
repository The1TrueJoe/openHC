// SPDX-License-Identifier: GPL-2.0-only
/*
 * Intel CE5300 (Groveland) general-purpose / fan PWM controller.
 *
 * One PCI function (8086:089f) with a single 256-byte MMIO BAR holding four PWM
 * channels at stride 0x20. There is no upstream driver and no public register
 * doc; the map below was reverse-engineered on the stock 3.12 kernel by driving
 * the vendor sysfs to 0/50/100 % and dumping the BAR at each level (see
 * board/ea-common/patches/PWM-CE5300-REGMAP.md in openHC):
 *
 *   per channel, offset = channel * 0x20:
 *     +0x00 CTRL    0x6000 = run, 0x0 = stop
 *     +0x04 DUTY    compare = duty_ns * 27MHz / 1e9; bit17 = 100 % full-on
 *     +0x08 PERIOD  count   = period_ns * 27MHz / 1e9
 *   tick clock = 27 MHz.
 *
 * On the Control4 EA1 the cooling fan is channel 2.
 */
#include <linux/bitops.h>
#include <linux/io.h>
#include <linux/math64.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/pwm.h>

#define CE_PWM_NCHAN		4
#define CE_PWM_STRIDE		0x20
#define CE_PWM_CTRL		0x00
#define CE_PWM_DUTY		0x04
#define CE_PWM_PERIOD		0x08

/* Values observed on the stock driver. Treat CTRL_RUN as a calibration knob:
 * it was constant (0x6000) across 50 % and 100 %, i.e. a pure enable/mode word. */
#define CE_PWM_CTRL_RUN		0x6000
#define CE_PWM_DUTY_FULL	BIT(17)		/* 100 % duty flag */

#define CE_PWM_HZ		27000000UL	/* 27 MHz tick clock */
#define CE_PWM_MAX_COUNT	0x1ffffU	/* period/duty count is 17 bits */

struct ce_pwm {
	void __iomem *base;
};

static void __iomem *ce_pwm_reg(struct ce_pwm *p, unsigned int ch, unsigned int reg)
{
	return p->base + ch * CE_PWM_STRIDE + reg;
}

/* nanoseconds -> tick count, saturating at the register width */
static u32 ce_pwm_ns_to_cnt(u64 ns)
{
	u64 cnt = div_u64(ns * CE_PWM_HZ, NSEC_PER_SEC);

	return cnt > CE_PWM_MAX_COUNT ? CE_PWM_MAX_COUNT : (u32)cnt;
}

static u64 ce_pwm_cnt_to_ns(u32 cnt)
{
	return div_u64((u64)cnt * NSEC_PER_SEC, CE_PWM_HZ);
}

static int ce_pwm_apply(struct pwm_chip *chip, struct pwm_device *pwm,
			const struct pwm_state *state)
{
	struct ce_pwm *p = pwmchip_get_drvdata(chip);
	unsigned int ch = pwm->hwpwm;
	u32 period, duty;

	if (state->polarity != PWM_POLARITY_NORMAL)
		return -EINVAL;

	if (!state->enabled) {
		writel(0, ce_pwm_reg(p, ch, CE_PWM_CTRL));
		writel(0, ce_pwm_reg(p, ch, CE_PWM_DUTY));
		return 0;
	}

	period = ce_pwm_ns_to_cnt(state->period);
	if (!period)
		return -EINVAL;
	writel(period, ce_pwm_reg(p, ch, CE_PWM_PERIOD));

	if (state->duty_cycle >= state->period)
		duty = CE_PWM_DUTY_FULL;		/* full-on */
	else
		duty = ce_pwm_ns_to_cnt(state->duty_cycle);
	writel(duty, ce_pwm_reg(p, ch, CE_PWM_DUTY));

	writel(CE_PWM_CTRL_RUN, ce_pwm_reg(p, ch, CE_PWM_CTRL));
	return 0;
}

static int ce_pwm_get_state(struct pwm_chip *chip, struct pwm_device *pwm,
			    struct pwm_state *state)
{
	struct ce_pwm *p = pwmchip_get_drvdata(chip);
	unsigned int ch = pwm->hwpwm;
	u32 ctrl = readl(ce_pwm_reg(p, ch, CE_PWM_CTRL));
	u32 period = readl(ce_pwm_reg(p, ch, CE_PWM_PERIOD));
	u32 duty = readl(ce_pwm_reg(p, ch, CE_PWM_DUTY));

	state->enabled = ctrl == CE_PWM_CTRL_RUN;
	state->polarity = PWM_POLARITY_NORMAL;
	state->period = ce_pwm_cnt_to_ns(period);
	state->duty_cycle = (duty & CE_PWM_DUTY_FULL) ? state->period
						      : ce_pwm_cnt_to_ns(duty);
	return 0;
}

static const struct pwm_ops ce_pwm_ops = {
	.apply = ce_pwm_apply,
	.get_state = ce_pwm_get_state,
};

static int ce_pwm_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct pwm_chip *chip;
	struct ce_pwm *p;
	int ret;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;

	chip = devm_pwmchip_alloc(&pdev->dev, CE_PWM_NCHAN, sizeof(*p));
	if (IS_ERR(chip))
		return PTR_ERR(chip);
	p = pwmchip_get_drvdata(chip);

	p->base = pcim_iomap_region(pdev, 0, "pwm-ce5300");
	if (IS_ERR(p->base))
		return PTR_ERR(p->base);

	chip->ops = &ce_pwm_ops;

	return devm_pwmchip_add(&pdev->dev, chip);
}

static const struct pci_device_id ce_pwm_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x089f) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce_pwm_ids);

static struct pci_driver ce_pwm_driver = {
	.name = "pwm-ce5300",
	.id_table = ce_pwm_ids,
	.probe = ce_pwm_probe,
};
builtin_pci_driver(ce_pwm_driver);

MODULE_DESCRIPTION("Intel CE5300 fan/GP PWM driver");
MODULE_LICENSE("GPL");
