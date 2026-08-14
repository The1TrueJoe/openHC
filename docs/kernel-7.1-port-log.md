# DM355 → Linux 7.1.8 port log

Working log for resurrecting TI DaVinci **DM355** support in mainline 7.1.8 for
the IO Extender V1 ("hammer"). Companion to the scoping doc
[`kernel-7.1-port.md`](kernel-7.1-port.md). Patches land in
`board/ioxv1/patches/linux/` (shared SoC) and
`board/ioxv1/patches/linux/` (board), applied by Buildroot at extract time.

## Decision record

- **Device tree, not a legacy board file.** Mainline's `mach-davinci` is DT-only
  (da850). The board-file infrastructure `dm355.c` relied on (common init, mux,
  board-file clock/irq wiring) was removed, so a board file fights the tree. The
  CCF clock drivers and every peripheral driver we need already have DT bindings
  (da850 proves it), so DT is the path of least resistance.
- **Boot with the stock U-Boot via appended DTB.** U-Boot 1.2.0-IOX passes ATAGS
  and boots a legacy uImage; it can't hand over a DTB. Use
  `CONFIG_ARM_APPENDED_DTB` (+ `CONFIG_ARM_ATAG_DTB_COMPAT` to fold the ATAG
  cmdline/memory in). Wrap the appended-DTB zImage as a uImage; `run tst`
  netboots it into RAM — no flash touched during bring-up.
- **Base defconfig:** `multi_v5` (ARMv5 multiplatform). DM355 = ARM926EJ-S.

## Mainline gap (verified against torvalds/master, 200=present / 404=removed)

Removed (resurrect from v6.1): `mach-davinci/dm355.c`, `clk/davinci/psc-dm355.c`,
`clk/davinci/pll-dm355.c`, `irqchip/irq-davinci-aintc.c`, `board-dm355-evm.c`.
Present (reuse): `da850.c`, `gpio-davinci`, `ti-aemif`, `davinci_nand`, `dm9000`,
`8250_of`, `dma/ti/edma`, `i2c-davinci`, `davinci_wdt`, the CCF clk framework
(`psc.c`/`pll.c`), cp-intc.

## Patch sequence

| # | Patch | Dir | Status |
|---|-------|-----|--------|
| 0001 | restore DM355 PSC+PLL clock drivers (DT) | ioxv1 | **compile+link verified (arm-linux-gnueabi, 7.1.8)** |
| 0002 | restore davinci AINTC irqchip + DT binding | ioxv1 | **compile verified (arm-linux-gnueabi, 7.1.8)** |
| 0003 | restore DM355 SoC (mach-davinci glue, Kconfig, DT_MACHINE) | ioxv1 | **compile+link verified (full zImage build)** |
| 0004 | dm355.dtsi + dm355-hammer.dts (first-boot: aintc, timer, console, 128M) | ioxv1 | **dtc+link verified (full zImage build)** |

**Milestone (0001–0004): the DM355 7.1.8 kernel builds.** All four patches apply
clean and `make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- zImage` on multi_v5 +
`ARCH_DAVINCI_DM355` links a 7.2 MB zImage with **zero undefined references** —
the AINTC `IRQCHIP_DECLARE`, the clock symbols, and the `DM355_DT` machine all
resolve, and `dm355-hammer.dtb` compiles. This is a real boot candidate; first
boot is the on-hardware serial-console step (no DM355 emulator exists).
| 0005 | dtsi peripherals: dm9000, NAND, i2c, gpio (relays/contacts) | ioxv1 | after console boot |
| 0006 | FPGA slave-serial loader (userspace libgpiod tool, rootfs) — not a kernel patch | ioxv1 overlay | after console boot |
| 0007 | IR-out driver (reverse c4irout.ko) | ioxv1 | after console boot |

## 0001 — clock drivers (done this pass)

- **What:** re-adds `psc-dm355.c` + `pll-dm355.c` verbatim from v6.1, adds DT
  init paths (`of_dm355_psc_init`, `of_dm355_pll1_init`) modeled on the surviving
  da850 drivers, registers `ti,dm355-psc` / `ti,dm355-pll1`, builds under
  `CONFIG_ARCH_DAVINCI_DM355`.
- **Why it's low-risk:** `drivers/clk/davinci/{psc,pll}.{c,h}` (the framework) is
  byte-identical v6.1→v7.1, so the descriptor tables need no change. Confirmed
  `of_davinci_pll_init` registers `pll1_auxclk` from a DT `auxclk` subnode and
  accepts a NULL obsclk — so the DM355 PLL (no obsclk) maps cleanly.
- **Verified (cross-compiled against Linux 7.1.8, `arm-linux-gnueabi-gcc`):**
  both new files build **warning-clean**; with DM355 off the davinci clk driver
  has **no dangling dm355 refs** (da850 stays link-clean); with DM355 on the
  objects build into `built-in.a` and the references resolve.
- **Bug found+fixed by the cross-build** (that `patch -p1` missed): the DT
  match/id-table entries reference `of_dm355_*` whose definitions only build under
  `CONFIG_ARCH_DAVINCI_DM355` (added by the SoC patch). Left unguarded they are
  undefined `U` symbols in `psc.o`/`pll.o` and break even a da850 vmlinux link.
  Fixed by `#ifdef CONFIG_ARCH_DAVINCI_DM355` around those entries.
- **Open risk:** DM355's clocksource may need PLL1 up at `CLK_OF_DECLARE` time
  (like da850's pll0) rather than via the platform driver — revisit when wiring
  the timer in 0003/0004.

## Verification loop (use this for every kernel patch)

A persistent Docker container cross-compiles patches in seconds — far faster than
a full Buildroot image build, and it catches link errors that `patch -p1` can't:

    docker run -d --name openhc-kbuild --memory=3g debian:bookworm-slim sleep infinity
    docker exec openhc-kbuild bash -c 'apt-get update -qq && apt-get install -y \
      gcc gcc-arm-linux-gnueabi make bc bison flex libssl-dev libelf-dev \
      wget xz-utils patch cpio ca-certificates'
    # fetch linux-7.1.8.tar.xz, extract, patch -p1 < 000N.patch
    # make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- multi_v5_defconfig
    # scripts/config -e ARCH_DAVINCI -e ARCH_DAVINCI_DA850 -e COMMON_CLK; make olddefconfig
    # make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- drivers/clk/davinci/  (etc.)
    # nm the objects to check symbol resolution both with the SoC symbol on and off

(NOTE: the kernel's HOSTCC needs a native `gcc` too, not just the cross-gcc.)

## 0002 — AINTC irqchip + DT binding (done this pass)

- **What:** restores `drivers/irqchip/irq-davinci-aintc.c` and its header from
  v6.1, refactors the init into `davinci_aintc_do_init()`, and adds a DT entry
  point `davinci_aintc_of_init` + `IRQCHIP_DECLARE(dm355_aintc, "ti,dm355-aintc")`.
  Wires `CONFIG_DAVINCI_AINTC` (hidden bool, selected by the SoC in 0003) into
  the irqchip Kconfig/Makefile.
- **The one real change from resurrection:** v6.1's AINTC was board-file only
  (`irq_domain_add_legacy(NULL, …)`), so DT nodes couldn't name it as
  `interrupt-parent`. The DT path creates an of_node-backed domain via
  `irq_domain_create_legacy(of_fwnode_handle(node), …)`, modeled on cp-intc.
  `irq_domain_simple_ops` already supplies `irq_domain_xlate_onetwocell`, so DT
  interrupt specifiers resolve. Base + `ti,intc-size` come from the node
  (defaults to 64 = DM355). Board-file `davinci_aintc_init()` kept for non-DT.
- **Verified:** applies -p1 on 0001; `irq-davinci-aintc.o` cross-compiles
  warning-clean against 7.1.8. Self-contained (only adds gated files), so no
  dangling-symbol risk.
- **DT node it expects (for 0004 dm355.dtsi):**
  `compatible = "ti,dm355-aintc"; reg = <0x01c48000 0x1000>; interrupt-controller;
   #interrupt-cells = <1>; ti,intc-size = <64>;`

## 0003/0004 — SoC + device tree (designed, extracted; the boot-critical pair)

This is where "resurrection" becomes real bring-up. Everything below is
researched and extracted from v6.1 + the living da850; what remains is writing
it, iterating full-kernel builds, and tuning on the actual serial console.

### Restoration scope (verified against torvalds/master)
- `arch/arm/mach-davinci/davinci.h` — **404, removed**. Restore the base defines.
- `arch/arm/mach-davinci/serial.h` — **404, removed**. Not needed if the board-file
  8250 devices are dropped (console comes from DT).
- `arch/arm/mach-davinci/irqs.h` — present, but DM355 IRQ numbers stripped; re-add.
- `arch/arm/mach-davinci/cputype.h` — present, `DAVINCI_CPU_ID_DM355` stripped; re-add.
- `mach-davinci/{Kconfig,Makefile}` — hardwired to da8xx: `ARCH_DAVINCI` force-selects
  `ARCH_DAVINCI_DA850`, and `da8xx-dt.o`/`devices-da8xx.o` are obj-y. Make DM355
  coexist (own bool, selects `DAVINCI_AINTC` + the clocks, NOT `ARCH_DAVINCI_DA8XX`).

### Extracted hardware values (DM355)
| what | value |
|------|-------|
| IO_PHYS | 0x01c00000 |
| System module | 0x01c40000 |
| PLL1 / PLL2 | 0x01c40800 / 0x01c40c00 |
| PSC (pwr/sleep) | 0x01c41000 |
| AINTC | 0x01c48000 (4K, 64 irqs) |
| Timer0 | 0x01c21400 |
| UART0 (console) | 0x01c20000, regshift 2 |
| GPIO | 0x01c67000 |
| ref clock | 24 MHz |
| timer IRQs | TINT12=32, TINT34=33 |
| UART0 IRQ | 40 |
| JTAG id | part 0xb73b → DAVINCI_CPU_ID_DM355 0x03550000 |

### The boot-critical decision: timer clock + IRQ consistency
Two facts collide:
1. DM355's **timer clock is behind the PSC**, a *late* platform driver — not up at
   `time_init`. (da850 dodges this: its timer clock is `pll0_auxclk`, and pll0 is
   `CLK_OF_DECLARE` = early.)
2. A **board-file timer** (`davinci_timer_register`, which *does* still exist in
   7.1) takes **fixed** IRQ numbers, but the **DT AINTC** allocates a *dynamic*
   `irq_base` — so fixed timer IRQ 32 won't match the domain. Mixing board-file
   timer with DT irqchip is inconsistent.

**Chosen path (fully-DT, consistent):** give DM355 **pll1 an early `CLK_OF_DECLARE`**
(like da850's pll0) exposing `pll1_auxclk` early; point the DT `timer@` node's
`clocks` at it; timer + all IRQs resolve through DT/AINTC. Requires adding an
early `of_dm355_pll1_init(struct device_node *)` variant (da850 pll0 is the model)
— a small addition to the 0001 PLL driver. Console UART uses a fixed
`clock-frequency = <24000000>` — **confirmed empirically** from the live 2.6.28
box (`/proc/davinci_clocks`: `UART0 24000000`) — so it needs no clock provider.
Clocks otherwise stay minimal for first boot; psc/pll DT nodes are added once the
early-pll path is proven, to avoid double registration.

### First-boot dtsi (0004) minimal node set
`memory` (128M), `chosen`/bootargs, `cpu` (arm926), `aintc@01c48000`
(ti,dm355-aintc, interrupt-controller, #interrupt-cells=1, ti,intc-size=64),
`ref_clk` + early `pll1`, `timer@01c21400` (ti,da830-timer, clocks=<&pll1_auxclk>),
`serial@01c20000` (ns16550a, reg-shift 2, clock-frequency=<…>, current-speed 115200).
Boot = appended DTB (CONFIG_ARM_APPENDED_DTB + ARM_ATAG_DTB_COMPAT), uImage @0x80008000,
`run tst`. dm9000/nand/i2c/gpio/fpga come after console is proven.

### Why this is staged, not dumped here
No DM355 emulation exists (not in QEMU), so first boot is an **on-hardware, serial-
console** exercise with expected iteration on exactly the ordering above. The plan
is: write 0003+0004, drive full-kernel builds in the `openhc-kbuild` container to a
clean uImage, then bring it up on the box over serial — that feedback loop is what
resolves the last unknowns (uartclk value, early-pll timing, DDR/ATAG handoff).

## Reproduce the reference sources

    L=https://raw.githubusercontent.com/torvalds/linux
    # removed DM355 files (resurrect these):
    for f in arch/arm/mach-davinci/dm355.c arch/arm/mach-davinci/board-dm355-evm.c \
             drivers/clk/davinci/psc-dm355.c drivers/clk/davinci/pll-dm355.c \
             drivers/irqchip/irq-davinci-aintc.c; do curl -sO "$L/v6.1/$f"; done
    # living DT reference:
    curl -s "$L/v7.1/arch/arm/boot/dts/ti/davinci/da850.dtsi"   # model dm355.dtsi on this
    curl -s "$L/v7.1/arch/arm/mach-davinci/da850.c"             # DT_MACHINE pattern

board-hammer.c (the 0005 board spec, Control4 GPL) is reconstructed at
`test/vendor-gpl/reconstructed/board-hammer.c`.

## Open technical questions (close as patches land)

- AINTC DT binding: v6.1 `irq-davinci-aintc.c` — did it have OF support, or is a
  new `davinci,aintc` binding needed? (0002)
- Timer/clocksource: which davinci timer node/driver 7.1 expects, and PLL early-init.
- EDMA: DM355 EDMA vs the `dma/ti/edma` DT binding (needed for NAND DMA, not for
  first boot — NAND works PIO).
- appended-DTB + ATAG cmdline: confirm the uImage load/entry (0x80008000) and that
  U-Boot's ATAGS memory tag is honored.
