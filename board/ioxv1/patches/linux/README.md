# DM355 → Linux 7.1.8 kernel patches (Buildroot GLOBAL_PATCH_DIR)

Buildroot applies every `*.patch` here to the kernel source at extract time
(numeric order). These resurrect TI DaVinci **DM355** support — removed from
mainline in v6.2 ("ARM: remove unused davinci board & drivers") — for the
Control4 IO Extender V1 ("hammer" board), and boot it from device tree. Full
status and rationale: `docs/kernel-7.1-port-log.md`.

- `0001-ARM-davinci-restore-dm355-clocks.patch`   **[cross-compile + link verified]**
      drivers/clk/davinci/psc-dm355.c, pll-dm355.c + DT init + guarded match
      tables. Framework API is byte-identical v6.1→v7.1; DT paths modeled on
      the surviving da850 drivers.
- `0002-irqchip-restore-davinci-aintc.patch`      **[cross-compile verified]**
      drivers/irqchip/irq-davinci-aintc.c + header. DM355 uses the AINTC (not
      cp-intc); given a DT interrupt-controller binding (ti,dm355-aintc) it
      never had upstream.
- `0003-ARM-davinci-restore-dm355-soc.patch`      **[compile + link verified]**
      Minimal mach-davinci/dm355.c (IO map + JTAG id), ARCH_DAVINCI_DM355
      (coexists with da850), the DM355_DT machine, DAVINCI_CPU_ID_DM355.
- `0004-ARM-dts-davinci-add-dm355-hammer.patch`   **[dtc + link verified]**
      dm355.dtsi + dm355-hammer.dts: AINTC, davinci timer on a fixed 24 MHz
      reference, ns16550a console (24 MHz, confirmed from the live 2.6.28 unit).

All four apply clean in sequence and link a 7.2 MB zImage on multi_v5 +
ARCH_DAVINCI_DM355 with zero undefined references (arm-linux-gnueabi, 7.1.8).

Reference sources: v6.1 (removed files) + the living da850, fetched per the
commands in `docs/kernel-7.1-port-log.md`. The reconstructed Control4
board-hammer.c (the on-device IO map spec) is staged at
`test/vendor-gpl/reconstructed/board-hammer.c`; the FPGA IR/serial + GPIO
relays/contacts nodes are added to the dtsi once console boot is confirmed.
