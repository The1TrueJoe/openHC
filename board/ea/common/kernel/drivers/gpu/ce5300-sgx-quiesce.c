// SPDX-License-Identifier: GPL-2.0
/*
 * ce5300-sgx-quiesce.c — hold the CE5300's SGX545 in soft reset before this
 * kernel hands the machine to another one (kexec) or to the firmware.
 *
 * Once the PVR driver (pvrsrvkm, the sgx545ce submodule) has probed the GPU at
 * 01:02.0, the SGX stays live after the driver lets go: its own shutdown hook
 * powers the core down through the microkernel, and even a full unbind leaves
 * it out of reset. A kernel kexec'd after that comes up broken, every time
 * (measured on an EA1, 2026-10-26):
 *
 *   - reads of the HPET (0xfed00000) and IOAPIC (0xfec00000) return the
 *     PREVIOUS read's value, so the HPET looks stopped ("clocksource: Watchdog
 *     hpet interval: 0ns", TSC marked unstable, timers die) and interrupt
 *     routing breaks;
 *   - DMA stalls: SDHCI "Timeout waiting for hardware interrupt", e1000
 *     "Detected Tx Unit Hang" (TDH stuck at 0), EHCI -110.
 *
 * It does not matter which kernel or initrd comes next. The same kexec from a
 * kernel where PVR never probed is clean, and so is one where the GPU never
 * rendered and EUR_CR_SOFT_RESET was then set to hold every unit (BIF, DPM, TA,
 * USE, ISP, TSP) in reset after PVR shut down. That write is what this file
 * does.
 *
 * It is NOT enough once the GPU has rendered (the panel web view ran). Then even
 * the reset plus every safely accessible SGX register back at its power-on value
 * (events cleared, BIF_CTRL 0xff00 = MMU bypass, CLKGATECTL 0) still leaves the
 * next kernel broken, with garbled bus-1 reads on top. A warm reboot through
 * CEFDK does not clear that state either; only a power-cycle does. So the
 * product never kexecs out of this kernel: ohc-restore reboots through CEFDK
 * into boot-init instead, which kexecs nothing.
 *
 * Why syscore_ops: kernel_kexec() and kernel_restart() run every device's
 * .shutdown first (PVR's D3 handshake with the microkernel needs the core out
 * of reset), then syscore_shutdown(), then hand off. So this runs after PVR
 * has finished, whichever SGX driver is in the image, and without changing it.
 *
 * Only when a driver is still bound to the SGX at that point: the lean
 * bootstrap kernel never binds one before S00kexec hands off, so the normal
 * CEFDK -> bootstrap -> real-kernel boot is unchanged. EUR_CR_SOFT_RESET is
 * one of the registers that is safe to access on this SGX in any state;
 * others (e.g. EUR_CR_USE_CODE_BASE_0, 0xa0c) hang the bus on an
 * uninitialised core. The next PVR probe deasserts the reset itself
 * (sgxreset.c).
 */
#include <linux/init.h>
#include <linux/io.h>
#include <linux/pci.h>
#include <linux/syscore_ops.h>

#define CE5300_SGX_DEVICE_ID		0x089b

/* SGX545 core registers (sgx545defs.h), BAR0 offset 0 on the CE5300 build. */
#define EUR_CR_SOFT_RESET		0x0080
#define EUR_CR_SOFT_RESET_ALL		0x3f	/* BIF DPM TA USE ISP TSP */

static void ce5300_sgx_quiesce(void)
{
	struct pci_dev *pdev;
	void __iomem *regs;
	u16 cmd;

	pdev = pci_get_device(PCI_VENDOR_ID_INTEL, CE5300_SGX_DEVICE_ID, NULL);
	if (!pdev)
		return;
	if (!pdev->dev.driver || !pci_resource_start(pdev, 0))
		goto out;

	/* PVR's own teardown leaves memory decode on and bus master off;
	 * make sure the register write lands either way. */
	pci_read_config_word(pdev, PCI_COMMAND, &cmd);
	if (!(cmd & PCI_COMMAND_MEMORY))
		pci_write_config_word(pdev, PCI_COMMAND, cmd | PCI_COMMAND_MEMORY);

	regs = ioremap(pci_resource_start(pdev, 0), PAGE_SIZE);
	if (!regs)
		goto out;
	writel(EUR_CR_SOFT_RESET_ALL, regs + EUR_CR_SOFT_RESET);
	readl(regs + EUR_CR_SOFT_RESET);	/* flush the posted write */
	iounmap(regs);
	pr_info("ce5300-sgx: held in soft reset for handoff\n");
out:
	pci_dev_put(pdev);
}

static struct syscore_ops ce5300_sgx_syscore_ops = {
	.shutdown	= ce5300_sgx_quiesce,
};

static int __init ce5300_sgx_quiesce_init(void)
{
	register_syscore_ops(&ce5300_sgx_syscore_ops);
	return 0;
}
device_initcall(ce5300_sgx_quiesce_init);
