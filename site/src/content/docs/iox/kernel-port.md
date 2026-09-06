---
title: The Linux 7.1 port
description: Resurrecting TI DaVinci DM355 support in mainline, patch by patch.
sidebar:
  order: 3
---

Mainline dropped DM355 support around v6.2. This is what it took to bring it back
far enough to boot Linux 7.1.8 on the hardware.

## The gap, verified against mainline

### Removed — must be resurrected from a ≤ v6.1 checkout

| File | Role |
|---|---|
| `arch/arm/mach-davinci/dm355.c` | SoC: device base addresses, clock wiring, pinmux, EDMA, timers |
| `arch/arm/mach-davinci/board-dm355-evm.c` | reference board — a template, not needed at runtime |
| `drivers/clk/davinci/psc-dm355.c` | power-sleep-controller clock data |
| `drivers/clk/davinci/pll-dm355.c` | PLL clock data |
| `drivers/irqchip/irq-davinci-aintc.c` | the AINTC interrupt controller (DM355 uses AINTC; da850 uses cp-intc) |

These slot into **frameworks that still exist**, because they are alive and
maintained for da850. That is adaptation, not from-scratch bring-up.

### Present in 7.1 — reuse as-is

`da850.c` (a living reference), `gpio-davinci`, `drivers/memory/ti-aemif.c`,
`davinci_nand`, `dm9000`, `8250`/`8250_of`, `dma/ti/edma`, `i2c-davinci`,
`davinci_wdt`, `clk/davinci/psc-da850.c`, cp-intc.

**Every peripheral driver the board needs already exists and has DT bindings.**
What was deleted is only the SoC glue.

Some headers were also stripped rather than removed wholesale:
`mach-davinci/davinci.h` and `serial.h` are gone entirely; `irqs.h` and
`cputype.h` are present but had their DM355 entries stripped and need them
re-added. And `mach-davinci/{Kconfig,Makefile}` are hardwired to da8xx —
`ARCH_DAVINCI` force-selects `ARCH_DAVINCI_DA850`, so DM355 has to be made to
coexist with its own bool that does *not* select `ARCH_DAVINCI_DA8XX`.

## Two decisions that shaped everything

### Device tree, not a legacy board file

The tempting shortcut was a board file: the stock U-Boot 1.2.0 passes ATAGS
natively, so no DTB and no bootloader changes would be needed.

It's the wrong call. Mainline's `mach-davinci` is **DT-only** now, the
board-file infrastructure `dm355.c` relied on (common init, mux, board-file clock
and IRQ wiring) was removed along with everything else, so a board file fights the
tree at every step. Meanwhile every peripheral driver we need already has a DT
binding.

The bootloader problem solves separately: `CONFIG_ARM_APPENDED_DTB` plus
`CONFIG_ARM_ATAG_DTB_COMPAT` appends the DTB to the zImage and folds the ATAG
cmdline and memory tag in. Wrap that as a legacy uImage and the stock U-Boot needs
no changes at all. Base defconfig is `multi_v5` — DM355 is ARM926EJ-S.

### The boot-critical collision: timer clock versus IRQ numbering

Two facts contradict each other:

1. **The DM355's timer clock is behind the PSC**, a *late* platform driver — not
   up at `time_init`. da850 dodges this entirely: its timer clock is
   `pll0_auxclk`, and pll0 is declared early.
2. **A board-file timer takes fixed IRQ numbers**, but the **DT AINTC allocates a
   dynamic IRQ base**, so a hardcoded timer IRQ of 32 won't match the domain.
   Mixing a board-file timer with a DT irqchip is inconsistent.

**Chosen path, fully DT, consistent:** give DM355's PLL1 an early
`CLK_OF_DECLARE` the way da850 does for pll0, exposing `pll1_auxclk` early, and
point the DT timer node's `clocks` at it. Timer and every IRQ then resolve through
DT and AINTC together.

The console UART sidesteps the problem with a fixed `clock-frequency`, and that
number isn't a guess — it was **confirmed empirically** on the live 2.6.28 box,
where `/proc/davinci_clocks` reports `UART0 24000000`.

## Extracted hardware values

| What | Value |
|---|---|
| IO_PHYS | `0x01c00000` |
| System module | `0x01c40000` |
| PLL1 / PLL2 | `0x01c40800` / `0x01c40c00` |
| PSC | `0x01c41000` |
| AINTC | `0x01c48000` (4K, 64 IRQs) |
| Timer0 | `0x01c21400` |
| UART0 (console) | `0x01c20000`, reg-shift 2 |
| GPIO | `0x01c67000` |
| Reference clock | 24 MHz |
| Timer IRQs | TINT12 = 32, TINT34 = 33 |
| UART0 IRQ | 40 |
| JTAG id | part `0xb73b` → `DAVINCI_CPU_ID_DM355` `0x03550000` |

## The patch sequence

| # | Patch | Status |
|---|---|---|
| 0001 | restore DM355 PSC + PLL clock drivers (DT) | **compile + link verified** |
| 0002 | restore davinci AINTC irqchip + DT binding | **compile verified** |
| 0003 | restore DM355 SoC (mach-davinci glue, Kconfig, DT_MACHINE) | **compile + link verified** |
| 0004 | `dm355.dtsi` + `dm355-hammer.dts` (first boot: aintc, timer, console, 128M) | **dtc + link verified, BOOTS** |
| 0005 | dtsi peripherals: dm9000, NAND, i2c, gpio | done — [SSH and GPIO work](/iox/) |
| 0006 | FPGA slave-serial loader (userspace libgpiod tool) | outstanding |
| 0007 | IR-out driver (reverse `c4irout.ko`) | outstanding |

**Milestone (0001–0004):** all four apply clean, a `multi_v5` build with
`ARCH_DAVINCI_DM355` links a 7.2 MB zImage with **zero undefined references** —
the AINTC `IRQCHIP_DECLARE`, the clock symbols and the `DM355_DT` machine all
resolve, and `dm355-hammer.dtb` compiles.

### 0001 — clock drivers

Re-adds `psc-dm355.c` and `pll-dm355.c` verbatim from v6.1 and adds DT init paths
modelled on the surviving da850 drivers.

**Why it is low-risk:** `drivers/clk/davinci/{psc,pll}.{c,h}`, the framework, is
**byte-identical v6.1 → v7.1**, so the descriptor tables need no change.
`of_davinci_pll_init` registers `pll1_auxclk` from a DT `auxclk` subnode and
accepts a NULL obsclk, so the DM355 PLL (which has no obsclk) maps cleanly.

:::caution[The bug the cross-build found that `patch -p1` couldn't]
The DT match and id-table entries reference `of_dm355_*` functions whose
definitions only build under `CONFIG_ARCH_DAVINCI_DM355`, which patch 0003 adds.
Left unguarded they are undefined symbols in `psc.o` and `pll.o`, and they break
**even a da850 vmlinux link**, on a configuration with nothing to do with our
board.

Fixed by `#ifdef`-guarding those entries. Every kernel patch in this tree is now
cross-built and symbol-checked with the SoC option both **on and off** before it
is believed.
:::

### 0002 — AINTC irqchip

Restores `irq-davinci-aintc.c` from v6.1, refactors the init into
`davinci_aintc_do_init()`, and adds a DT entry point plus
`IRQCHIP_DECLARE(dm355_aintc, "ti,dm355-aintc")`.

**The one real change from a straight resurrection:** v6.1's AINTC was board-file
only — `irq_domain_add_legacy(NULL, …)` — so DT nodes couldn't name it as
`interrupt-parent` at all. The DT path creates an of_node-backed domain via
`irq_domain_create_legacy(of_fwnode_handle(node), …)`, modelled on cp-intc.
`irq_domain_simple_ops` already supplies `irq_domain_xlate_onetwocell`, so DT
interrupt specifiers resolve. Base and size come from the node, defaulting to 64.
The board-file entry point is kept for non-DT use.

The node it expects:

```
compatible = "ti,dm355-aintc";
reg = <0x01c48000 0x1000>;
interrupt-controller;
#interrupt-cells = <1>;
ti,intc-size = <64>;
```

### 0004 — the first-boot device tree

Minimal node set: `memory` (128M), `chosen`/bootargs, `cpu` (arm926),
`aintc@01c48000`, `ref_clk` plus the early `pll1`, `timer@01c21400`
(`ti,da830-timer`, clocked from `pll1_auxclk`), and `serial@01c20000`
(`ns16550a`, reg-shift 2, fixed `clock-frequency`, 115200).

Boot is the appended DTB, uImage at `0x80008000`, via `run tst`.

Clocks stay minimal for first boot: the psc and pll DT nodes are added only once
the early-pll path is proven, to avoid double registration.

## The verification loop

A persistent Docker container cross-compiles patches in **seconds**, which is far
faster than a full Buildroot image build and catches link errors `patch -p1`
cannot see:

```sh
docker run -d --name openhc-kbuild --memory=3g debian:bookworm-slim sleep infinity
docker exec openhc-kbuild bash -c 'apt-get update -qq && apt-get install -y \
  gcc gcc-arm-linux-gnueabi make bc bison flex libssl-dev libelf-dev \
  wget xz-utils patch cpio ca-certificates'
# fetch linux-7.1.8.tar.xz, extract, patch -p1 < 000N.patch
# make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- multi_v5_defconfig
# scripts/config -e ARCH_DAVINCI -e ARCH_DAVINCI_DA850 -e COMMON_CLK; make olddefconfig
# make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- drivers/clk/davinci/
# nm the objects to check symbol resolution both with the SoC symbol on and off
```

The kernel's `HOSTCC` needs a **native** `gcc` too, not just the cross compiler.

## Why this had to be staged

**No DM355 emulation exists**, not in QEMU, so first boot is an on-hardware,
serial-console exercise. The plan was to drive full-kernel builds in the container
to a clean uImage and then bring it up on the box, because that feedback loop is
the only thing that could resolve the last unknowns: the UART clock value, the
early-PLL timing, and the ATAG handoff.

It did. See [the current state](/iox/).

## Reproducing the reference sources

```sh
L=https://raw.githubusercontent.com/torvalds/linux
# removed DM355 files (resurrect these):
for f in arch/arm/mach-davinci/dm355.c arch/arm/mach-davinci/board-dm355-evm.c \
         drivers/clk/davinci/psc-dm355.c drivers/clk/davinci/pll-dm355.c \
         drivers/irqchip/irq-davinci-aintc.c; do curl -sO "$L/v6.1/$f"; done
# living DT reference:
curl -s "$L/v7.1/arch/arm/boot/dts/ti/davinci/da850.dtsi"
curl -s "$L/v7.1/arch/arm/mach-davinci/da850.c"
```

## Open technical questions

- **The FPGA UART input clock**, needed for the 8250 `uartclk`. From
  `c4serial.ko`, or measure it.
- **IR-out register semantics**, from `c4irout.ko`.
- **EDMA**: DM355 EDMA against the `dma/ti/edma` DT binding. Needed for NAND DMA,
  not for boot — NAND works in PIO.

## Effort, for anyone repeating this

The original scoping estimate was **4–6 weeks to Ethernet, all digital IO and four
serial ports, plus 1–3 weeks for IR** for one embedded-Linux engineer with a
serial console. Ethernet, digital IO and the kernel came in around that; serial
and IR are still gated on the FPGA loader rather than on the kernel.
