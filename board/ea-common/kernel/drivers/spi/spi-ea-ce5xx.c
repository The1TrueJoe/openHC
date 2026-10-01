// SPDX-License-Identifier: GPL-2.0
/*
 * spi-ea-ce5xx.c — SPI-mem controller for the Intel CE5300/CE2600 serial-flash
 * block, the DEDICATED boot-flash controller on Control4 EA machines.
 *
 * PCI 8086:08A0, class 0501 "FLASH memory", function 01:17.0. This is NOT the
 * general-purpose pxa2xx SSP (8086:2e6a) that the BCM switch rides — the boot
 * SPI-NOR (an S25FL127S, 16 MiB, where CEFDK + the Master Flash Header + the
 * openHC boot autoscript live) hangs off its own block with three PCI BARs:
 *
 *   BAR0  (0xdffe0100, 256 B)   command/status registers (CSR)
 *   BAR1  (0xd8000000, 64 MiB)  a direct read window into the flash array
 *   BAR2  (0xdffe0000, 256 B)   secondary regs (unused here)
 *
 * The hardware is a command-register sequencer, not a raw shift engine: you
 * write an opcode+data unit (<=3 bytes, big-endian) into DATA_COMMAND_REG with
 * the CS-HOLD bit to keep /CS asserted across units, and read reply bytes back
 * out of the same register. Bulk reads of the main array are far faster through
 * the 64 MiB window (a plain memcpy_fromio) once the address-split register is
 * told how the window maps onto the chip.
 *
 * This is a mainline-API port of Intel's out-of-tree ce5xx_spi_flash.c (GPL,
 * (c) 2011-2013 Intel) rewritten against the spi-mem exec_op interface so that
 * the in-tree spi-nor driver detects and drives the chip by JEDEC id and openHC
 * gets a /dev/mtd0 — the prerequisite for rewriting its own boot autoscript and
 * for a software restore of the stock firmware. The register map, the big-endian
 * unit packing and the clock/mode setup are taken verbatim from that driver; the
 * legacy per-message workqueue and the companion nmyx25 MTD chip driver are
 * dropped (spi-mem provides the message pump, spi-nor provides the chip).
 *
 * The HW_MUTEX the vendor used to arbitrate the flash between the Linux core and
 * the other CE5300 cores is intentionally omitted: under openHC Linux is the
 * only thing running after CEFDK hands off, so a plain controller lock suffices.
 */
#include <linux/delay.h>
#include <linux/init.h>
#include <linux/io.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/spi/spi.h>
#include <linux/spi/spi-mem.h>

/* --- PCI identity --------------------------------------------------------- */
#define CE5XX_FLASH_DEVICE_ID		0x08a0

/* --- CSR register map (BAR0), from ce5xx_spi_flash.h ---------------------- */
#define MODE_CONTL_REG			0x00
#define   MODE_CONTL_CLK_RATIOR_SHIFT	0
#define   MODE_CONTL_BOOT_MODE_ENABLE	(1 << 4)
#define   MODE_CONTL_SPI_UNIT_EN	(1 << 5)
#define   MODE_CONTL_SS1_EN		(1 << 6)
#define   MODE_CONTL_CMD_WIDTH_EQUAL_TO_DATA	(0 << 9)
#define   MODE_CONTL_SPI_WIDTH_1_BIT	(1 << 10)
#define   MODE_CONTL_SPI_WIDTH_BIT_MASK	(~(3 << 10))
#define   MODE_CONTL_N_ADDR_3_BYTES	(3 << 12)
#define   MODE_CONTL_N_ADDR_4_BYTES	(4 << 12)
#define   MODE_CONTL_N_ADDR_BYTES_MASK	(~(0xf << 12))
#define   MODE_CONTL_CS0_MODE_ENABLE	(1 << 16)
#define   MODE_CONTL_CS0_WP		(1 << 18)
#define   MODE_CONTL_CS1_MODE_ENABLE	(1 << 20)
#define   MODE_CONTL_CS1_WP		(1 << 22)
#define   MODE_CONTL_CS_TAR_SHIFT	24

#define ADDR_SPLIT_REG			0x04
#define   CS0_CMP			0
#define   CS0_MASK			4
#define   CS1_CMP			8
#define   CS1_MASK			12

#define CURRENT_ADDR_REG		0x08

#define DATA_COMMAND_REG		0x0c
#define   DCR_CS_HOLD			26
#define   DCR_NBYTES			24
#define   DCR_NBYTES_MAX		3

/* Main-array read opcodes that may use the fast 64 MiB window. Everything else
 * (RDID, RDSR, SFDP 0x5A, WREN, PP, erases, register writes) goes through the
 * CSR path, because the window only ever returns main-array data. */
static bool ce5xx_is_array_read(u8 op)
{
	return op == 0x03 || op == 0x0b ||	/* READ, FAST_READ (3-byte) */
	       op == 0x13 || op == 0x0c;	/* READ4, FAST_READ4 (4-byte) */
}

struct ce5xx_spi {
	struct spi_controller	*ctlr;
	struct pci_dev		*pdev;
	void __iomem		*regs;		/* BAR0 CSR */
	void __iomem		*mem;		/* BAR1 64 MiB window */
};

/* /CS goes high: a zero-length DATA_COMMAND with CS-HOLD clear. */
static void ce5xx_cs_off(struct ce5xx_spi *c)
{
	writel(0, c->regs + DATA_COMMAND_REG);
}

/* Only CS0 is populated on EA (the boot flash); CS1 is the unused second
 * select. Enable exactly the one this op targets. */
static void ce5xx_cs_select(struct ce5xx_spi *c, u8 cs)
{
	u32 v = readl(c->regs + MODE_CONTL_REG);

	if (cs == 0)
		v = (v | MODE_CONTL_CS0_MODE_ENABLE) & ~MODE_CONTL_CS1_MODE_ENABLE;
	else
		v = (v | MODE_CONTL_CS1_MODE_ENABLE) & ~MODE_CONTL_CS0_MODE_ENABLE;

	writel(v, c->regs + MODE_CONTL_REG);
}

/*
 * Write up to 3 bytes as one CS-held unit. Data is packed big-endian into the
 * low 24 bits of DATA_COMMAND_REG, exactly as the vendor driver does: the chip
 * sees byte[0] first on the wire.
 */
static void ce5xx_unit_write(struct ce5xx_spi *c, const u8 *buf, unsigned int len)
{
	u32 v = 0;
	unsigned int i;

	/* Data is LEFT-aligned in the 24-bit field: byte[0] always at bits
	 * 23:16, byte[1] at 15:8, byte[2] at 7:0, whatever len is. (The vendor
	 * gets this via cpu_to_be32(packed) >> 8; spelled out here so a partial
	 * final chunk, len < 3, lands in the same high bytes the engine shifts
	 * out first.) */
	for (i = 0; i < len; i++)
		v |= (u32)buf[i] << (8 * (DCR_NBYTES_MAX - 1 - i));

	v |= (1u << DCR_CS_HOLD) | (len << DCR_NBYTES);
	writel(v, c->regs + DATA_COMMAND_REG);
}

/*
 * Clock out up to 3 dummy/read bytes (CS held) and return them. For a pure read
 * the TX line is don't-care; nbytes drives how many bytes are shifted. Reply is
 * big-endian in the low bytes of DATA_COMMAND_REG.
 */
static void ce5xx_unit_read(struct ce5xx_spi *c, u8 *buf, unsigned int len)
{
	u32 v;
	unsigned int i;

	writel((1u << DCR_CS_HOLD) | (len << DCR_NBYTES), c->regs + DATA_COMMAND_REG);
	v = readl(c->regs + DATA_COMMAND_REG);		/* byte[len-1] in LSB */

	for (i = 0; i < len; i++)
		buf[len - 1 - i] = (v >> (8 * i)) & 0xff;
}

static void ce5xx_write_bytes(struct ce5xx_spi *c, const u8 *buf, size_t len)
{
	while (len) {
		unsigned int n = min_t(size_t, len, DCR_NBYTES_MAX);

		ce5xx_unit_write(c, buf, n);
		buf += n;
		len -= n;
	}
}

static void ce5xx_read_bytes(struct ce5xx_spi *c, u8 *buf, size_t len)
{
	while (len) {
		unsigned int n = min_t(size_t, len, DCR_NBYTES_MAX);

		ce5xx_unit_read(c, buf, n);
		buf += n;
		len -= n;
	}
}

static int ce5xx_exec_op(struct spi_mem *mem, const struct spi_mem_op *op)
{
	struct ce5xx_spi *c = spi_controller_get_devdata(mem->spi->controller);
	u8 cs = spi_get_chipselect(mem->spi, 0);
	u8 cmd[8];
	unsigned int i, n = 0;

	/* Fast path: bulk read of the main array through the memory window. */
	if (ce5xx_is_array_read(op->cmd.opcode) &&
	    op->addr.nbytes && op->data.nbytes &&
	    op->data.dir == SPI_MEM_DATA_IN) {
		if (op->addr.val + op->data.nbytes > resource_size(&c->pdev->resource[1]))
			return -EINVAL;
		memcpy_fromio(op->data.buf.in, c->mem + op->addr.val,
			      op->data.nbytes);
		return 0;
	}

	/* CSR path: opcode + address [+ dummy] + data, /CS held throughout. */
	ce5xx_cs_off(c);
	ce5xx_cs_select(c, cs);

	cmd[n++] = op->cmd.opcode;
	for (i = 0; i < op->addr.nbytes; i++)
		cmd[n++] = op->addr.val >> (8 * (op->addr.nbytes - 1 - i));
	ce5xx_write_bytes(c, cmd, n);

	/* Dummy bytes: clock them out as zero writes while /CS stays asserted. */
	if (op->dummy.nbytes) {
		u8 z[8] = { 0 };
		size_t d = op->dummy.nbytes;

		while (d) {
			unsigned int k = min_t(size_t, d, sizeof(z));

			ce5xx_write_bytes(c, z, k);
			d -= k;
		}
	}

	if (op->data.nbytes) {
		if (op->data.dir == SPI_MEM_DATA_OUT)
			ce5xx_write_bytes(c, op->data.buf.out, op->data.nbytes);
		else
			ce5xx_read_bytes(c, op->data.buf.in, op->data.nbytes);
	}

	ce5xx_cs_off(c);
	return 0;
}

static bool ce5xx_supports_op(struct spi_mem *mem, const struct spi_mem_op *op)
{
	/* Single-bit (x1) only; the EA flash is wired in legacy SPI mode. */
	if (op->cmd.buswidth > 1 || op->addr.buswidth > 1 ||
	    op->dummy.buswidth > 1 || op->data.buswidth > 1)
		return false;
	if (op->addr.nbytes > 4)
		return false;
	return spi_mem_default_supports_op(mem, op);
}

static const struct spi_controller_mem_ops ce5xx_mem_ops = {
	.supports_op	= ce5xx_supports_op,
	.exec_op	= ce5xx_exec_op,
};

/*
 * Put the controller in the mode CEFDK leaves it in and the flash expects:
 * legacy x1, 3-byte addressing, boot mode, both chip-select windows enabled.
 * Clock ratio 0x2 (~16.7 MHz): the vendor notes the FPGA-fronted flash is not
 * reliable at the 33 MHz that ratio 0x1 gives, and read correctness matters far
 * more than speed on a part we write our own boot header into.
 */
static void ce5xx_hw_init(struct ce5xx_spi *c)
{
	u32 v = readl(c->regs + MODE_CONTL_REG);

	v = (v >> 3 << 3) | (0x2 << MODE_CONTL_CLK_RATIOR_SHIFT);
	v |= MODE_CONTL_BOOT_MODE_ENABLE | MODE_CONTL_SS1_EN;
	v |= MODE_CONTL_CMD_WIDTH_EQUAL_TO_DATA;
	v = (v & MODE_CONTL_SPI_WIDTH_BIT_MASK) | MODE_CONTL_SPI_WIDTH_1_BIT;
	v = (v & MODE_CONTL_N_ADDR_BYTES_MASK) | MODE_CONTL_N_ADDR_3_BYTES;
	v |= MODE_CONTL_CS0_MODE_ENABLE | MODE_CONTL_CS0_WP |
	     MODE_CONTL_CS1_MODE_ENABLE | MODE_CONTL_CS1_WP;
	v |= 0x4 << MODE_CONTL_CS_TAR_SHIFT;
	writel(v, c->regs + MODE_CONTL_REG);

	/* Address-split for a single 16 MiB device on CS0 (addr_split_tbl entry
	 * {SIZE_16_MB,0, cs0_cmp=0x8,...}): map the window's low 16 MiB to CS0. */
	writel((0x8u << CS0_CMP), c->regs + ADDR_SPLIT_REG);
}

static struct spi_board_info ce5xx_flash_info = {
	.modalias	= "spi-nor",
	.chip_select	= 0,
	.max_speed_hz	= 16670000,
	.mode		= SPI_MODE_0,
};

static int ce5xx_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct spi_controller *ctlr;
	struct ce5xx_spi *c;
	struct spi_device *flash;
	void __iomem * const *iomap;
	int ret;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;

	/* Map BAR0 (CSR) and BAR1 (64 MiB window); both device-managed. */
	ret = pcim_iomap_regions(pdev, BIT(0) | BIT(1), "spi-ea-ce5xx");
	if (ret)
		return ret;
	iomap = pcim_iomap_table(pdev);
	if (!iomap)
		return -ENOMEM;

	ctlr = devm_spi_alloc_host(&pdev->dev, sizeof(*c));
	if (!ctlr)
		return -ENOMEM;

	c = spi_controller_get_devdata(ctlr);
	c->ctlr = ctlr;
	c->pdev = pdev;
	c->regs = iomap[0];
	c->mem = iomap[1];

	/* The SPI I/O unit is powered down by BIOS on some SKUs; without it the
	 * controller cannot talk to the flash at all, so fail loudly here. */
	if (!(readl(c->regs + MODE_CONTL_REG) & MODE_CONTL_SPI_UNIT_EN)) {
		dev_err(&pdev->dev, "SPI unit disabled by firmware; no flash\n");
		return -ENODEV;
	}

	ce5xx_hw_init(c);

	ctlr->dev.parent	= &pdev->dev;
	ctlr->bus_num		= -1;		/* dynamic */
	ctlr->num_chipselect	= 2;
	ctlr->mode_bits		= SPI_MODE_0;
	ctlr->bits_per_word_mask = SPI_BPW_MASK(8);
	ctlr->mem_ops		= &ce5xx_mem_ops;
	ctlr->flags		= SPI_CONTROLLER_HALF_DUPLEX;
	pci_set_drvdata(pdev, c);

	ret = devm_spi_register_controller(&pdev->dev, ctlr);
	if (ret)
		return ret;

	/* No device tree on this machine (CEFDK passes no DTB), so instantiate
	 * the flash by hand on CS0; spi-nor then autodetects it by JEDEC id. */
	flash = spi_new_device(ctlr, &ce5xx_flash_info);
	if (!flash)
		dev_warn(&pdev->dev, "could not add spi-nor device on CS0\n");

	dev_info(&pdev->dev, "CE5300 boot SPI-NOR controller ready (CSR %pR, win %pR)\n",
		 &pdev->resource[0], &pdev->resource[1]);
	return 0;
}

static const struct pci_device_id ce5xx_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, CE5XX_FLASH_DEVICE_ID) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce5xx_ids);

static struct pci_driver ce5xx_driver = {
	.name		= "spi-ea-ce5xx",
	.id_table	= ce5xx_ids,
	.probe		= ce5xx_probe,
};

static int __init ce5xx_init(void)
{
	return pci_register_driver(&ce5xx_driver);
}
/* subsys_initcall so the master (and the spi-nor child it spawns) are up before
 * the MTD users that want /dev/mtd0, mirroring the other EA board drivers. */
subsys_initcall(ce5xx_init);

MODULE_DESCRIPTION("Intel CE5300/CE2600 boot SPI-NOR controller (Control4 EA)");
MODULE_LICENSE("GPL");
