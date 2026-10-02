// SPDX-License-Identifier: GPL-2.0
/*
 * GPIO driver for the Intel CE5300 (and CE4100/CE4200) "public" GPIO block.
 *
 * The CE5300 exposes 4 banks of 32 GPIOs behind PCI 8086:2e67 (BAR 0). Each
 * bank is 0x20 bytes of MMIO: OUT(0x00) / OUT_EN(0x04) / INPUT(0x08) plus
 * interrupt registers we do not use here. This is the controller that carries
 * the named board lines the vendor kernel exported under /dev/gpio/
 * (codec_reset=24, dsp_reset=101, the front-panel LEDs, ...).
 *
 * Mainline's gpio-sodaville binds the same 8086:2e67 but with the CE4100 layout
 * (12 lines), which is wrong for the CE5300 — so this driver replaces it (build
 * with CONFIG_GPIO_SODAVILLE=n). Register model reconstructed from Control4's
 * GPL kernel drop (drivers/gpio/{intelce,ce5300,ce4100}-gpio.c, GPL-2.0).
 *
 * Pin muxing (CE5300_PUB_GPIO_MUX_CTL) is intentionally NOT touched: CEFDK
 * already muxes the board's GPIO lines (resets, LEDs) to GPIO mode before Linux
 * runs, and blindly re-muxing a pin that is currently a UART/PWM would break it.
 * Add per-pin mux only if a needed line turns out not to be GPIO-muxed.
 */
#include <linux/gpio/driver.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/spinlock.h>

#define INTELCE_GPIO_BAR	0
#define BANK_STRIDE		0x20
#define GPIOS_PER_BANK		32

#define REG_OUT			0x00
#define REG_OUT_EN		0x04
#define REG_INPUT		0x08

struct intelce_gpio {
	struct gpio_chip chip;
	void __iomem *base;
	spinlock_t lock;
};

static inline void __iomem *bank_reg(struct intelce_gpio *g, unsigned off, u32 reg)
{
	return g->base + (off / GPIOS_PER_BANK) * BANK_STRIDE + reg;
}
#define BIT_OF(off)	BIT((off) % GPIOS_PER_BANK)

static int intelce_gpio_get(struct gpio_chip *chip, unsigned off)
{
	struct intelce_gpio *g = gpiochip_get_data(chip);

	return !!(readl(bank_reg(g, off, REG_INPUT)) & BIT_OF(off));
}

static int intelce_gpio_set(struct gpio_chip *chip, unsigned off, int val)
{
	struct intelce_gpio *g = gpiochip_get_data(chip);
	void __iomem *reg = bank_reg(g, off, REG_OUT);
	unsigned long flags;
	u32 v;

	spin_lock_irqsave(&g->lock, flags);
	v = readl(reg);
	if (val)
		v |= BIT_OF(off);
	else
		v &= ~BIT_OF(off);
	writel(v, reg);
	spin_unlock_irqrestore(&g->lock, flags);
	return 0;
}

static int intelce_gpio_direction_input(struct gpio_chip *chip, unsigned off)
{
	struct intelce_gpio *g = gpiochip_get_data(chip);
	void __iomem *reg = bank_reg(g, off, REG_OUT_EN);
	unsigned long flags;

	spin_lock_irqsave(&g->lock, flags);
	writel(readl(reg) & ~BIT_OF(off), reg);
	spin_unlock_irqrestore(&g->lock, flags);
	return 0;
}

static int intelce_gpio_direction_output(struct gpio_chip *chip, unsigned off, int val)
{
	struct intelce_gpio *g = gpiochip_get_data(chip);
	void __iomem *reg = bank_reg(g, off, REG_OUT_EN);
	unsigned long flags;

	intelce_gpio_set(chip, off, val);
	spin_lock_irqsave(&g->lock, flags);
	writel(readl(reg) | BIT_OF(off), reg);
	spin_unlock_irqrestore(&g->lock, flags);
	return 0;
}

static int intelce_gpio_get_direction(struct gpio_chip *chip, unsigned off)
{
	struct intelce_gpio *g = gpiochip_get_data(chip);

	/* OUT_EN bit set => output */
	if (readl(bank_reg(g, off, REG_OUT_EN)) & BIT_OF(off))
		return GPIO_LINE_DIRECTION_OUT;
	return GPIO_LINE_DIRECTION_IN;
}

static int intelce_gpio_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct intelce_gpio *g;
	int ret;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;

	g = devm_kzalloc(&pdev->dev, sizeof(*g), GFP_KERNEL);
	if (!g)
		return -ENOMEM;
	spin_lock_init(&g->lock);

	g->base = pcim_iomap(pdev, INTELCE_GPIO_BAR, 0);
	if (!g->base)
		return -ENOMEM;

	g->chip.label			= "intelce-gpio";
	g->chip.parent			= &pdev->dev;
	g->chip.owner			= THIS_MODULE;
	g->chip.base			= 0;	/* match the vendor numbering (gpio24=codec_reset, ...) */
	g->chip.ngpio			= 4 * GPIOS_PER_BANK;	/* CE5300: 4 banks x 32 = 128 */
	g->chip.get			= intelce_gpio_get;
	g->chip.set			= intelce_gpio_set;
	g->chip.direction_input		= intelce_gpio_direction_input;
	g->chip.direction_output	= intelce_gpio_direction_output;
	g->chip.get_direction		= intelce_gpio_get_direction;
	g->chip.can_sleep		= false;

	ret = devm_gpiochip_add_data(&pdev->dev, &g->chip, g);
	if (ret)
		return ret;

	dev_info(&pdev->dev, "CE5300 GPIO: %d lines on BAR%d\n",
		 g->chip.ngpio, INTELCE_GPIO_BAR);
	return 0;
}

static const struct pci_device_id intelce_gpio_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x2e67) },
	{ }
};
MODULE_DEVICE_TABLE(pci, intelce_gpio_ids);

static struct pci_driver intelce_gpio_driver = {
	.name		= "intelce-gpio",
	.id_table	= intelce_gpio_ids,
	.probe		= intelce_gpio_probe,
};
module_pci_driver(intelce_gpio_driver);

MODULE_DESCRIPTION("Intel CE5300 public GPIO controller");
MODULE_LICENSE("GPL");
