---
title: Resurrecting a dead SoC
topic: Kernel
summary: Mainline deleted the DM355. Four files came back out of git history, and then eight relays refused to click for a reason no amount of GPIO debugging would have found.
description: Porting Linux 7.1.8 to the TI DaVinci DM355, and the pinmux discovery that made the IO work.
sidebar:
  order: 8
  label: Resurrecting a dead SoC
---

The IO Extender V1 is the board this project started for. It's also the only one
whose SoC no longer exists in Linux: mainline dropped TI DaVinci DM355 support
around v6.2.

The obvious reading of that's "port a 2009 kernel forward" or "stay on 2.6.28
forever". Both are wrong, and the gap analysis is why.

## What was actually removed

A file-by-file check against mainline found that only the SoC glue was deleted.
Five files, four of which we need:

| File | Role |
|---|---|
| `arch/arm/mach-davinci/dm355.c` | device base addresses, clock wiring, pinmux, timers |
| `drivers/clk/davinci/psc-dm355.c` | power-sleep-controller clock data |
| `drivers/clk/davinci/pll-dm355.c` | PLL clock data |
| `drivers/irqchip/irq-davinci-aintc.c` | the AINTC interrupt controller |

Every peripheral driver the board needs is still in 7.1 — `gpio-davinci`,
`ti-aemif`, `davinci_nand`, `dm9000`, `8250`, `edma`, `i2c-davinci`,
`davinci_wdt`, because the da850 platform keeps the whole DaVinci subsystem alive
and maintained. The frameworks the deleted files slot into are still there too.

So this is adaptation into living code, not from-scratch bring-up. That reframing
is what turned an open-ended job into a scoped one.

## Device tree, not a board file

The tempting shortcut was a legacy board file. The stock U-Boot 1.2.0 passes ATAGS
natively and can't hand over a DTB, and I already had the vendor's
`board-hammer.c` as a spec.

It was the wrong call, and the reason is structural. Mainline's `mach-davinci` is
DT-only now. The board-file infrastructure `dm355.c` relied on, common init, mux,
board-file clock and IRQ wiring, was removed along with everything else, so a
board file fights the tree at every step. Meanwhile every peripheral driver
already has a DT binding, because da850 proves it.

The bootloader problem solves separately. `CONFIG_ARM_APPENDED_DTB` plus
`CONFIG_ARM_ATAG_DTB_COMPAT` appends the DTB to the zImage and folds the ATAG
cmdline and memory tag in. Wrap that as a legacy uImage and the stock U-Boot needs
no changes at all.

## The collision that decided the design

Two facts about this SoC contradict each other, and resolving it was the
boot-critical decision.

The DM355's timer clock is behind the PSC, which is a *late* platform driver, not
up at `time_init`. (da850 dodges this entirely: its timer clock is `pll0_auxclk`,
and pll0 is declared early.)

But a board-file timer takes fixed IRQ numbers, while a DT AINTC allocates a
dynamic IRQ base, so a hardcoded timer IRQ of 32 won't match the domain. Mixing a
board-file timer with a DT irqchip is just inconsistent.

The path I took is fully-DT and consistent: give the DM355's PLL1 an early
declaration the way da850 does for pll0, exposing `pll1_auxclk` early, and point
the DT timer node's `clocks` at it. Timer and every IRQ then resolve through DT
and AINTC together.

The console UART sidesteps the problem with a fixed `clock-frequency`, and that
number isn't a guess. I read it off the live 2.6.28 box, where
`/proc/davinci_clocks` reports `UART0 24000000`.

## The bug the cross-build found that patching couldn't

The resurrection landed as four patches, verified in a persistent Docker container
that cross-compiles in seconds rather than through a full Buildroot image build.
That loop paid for itself immediately.

The clock patch applied cleanly and looked correct. It wasn't. The DT match and
id-table entries reference `of_dm355_*` functions whose definitions only build
under `CONFIG_ARCH_DAVINCI_DM355`, which a later patch adds. Left unguarded
they're undefined symbols in `psc.o` and `pll.o`, and they break even a da850
vmlinux link, on a configuration that has nothing to do with this board.

`patch -p1` can't see that. A compile can. Every kernel patch in this tree now
gets cross-built and symbol-checked with the SoC option both on and off before I
believe it.

The AINTC needed one genuine change rather than a straight restore. The v6.1
driver was board-file only, creating its interrupt domain with a NULL of_node, so
DT nodes couldn't name it as `interrupt-parent` at all. The DT path creates an
of_node-backed domain instead, modelled on the surviving cp-intc driver.

All four patches apply clean, a `multi_v5` build with `ARCH_DAVINCI_DM355` links a
zImage with zero undefined references, and the board DTB compiles.

## And then it booted

Linux 7.1.8 reached userspace and printed `openhc-ioxv1 login:` on real DM355
silicon. Then Ethernet and SSH came up, and two more findings came with them.

**GPIO has to be banked in the device tree.** With
`ti,davinci-gpio-unbanked = <0>` and seven bank IRQs wired to AINTC, everything
works. In unbanked mode the driver registers no IRQ domain at all, so `dm9000`,
which names the GPIO controller as its interrupt parent, can't resolve its
interrupt and fails with `-517`, "IRQ index 0 not found". The error names an IRQ;
the cause is a domain that was never created.

**libgpiod 1.6.4 needs the v1 character-device ABI**, `CONFIG_GPIO_CDEV_V1`, which
defaults off in 7.1.8. Without it `gpioset`, `gpioget` and `gpioinfo` all return
`EINVAL`. Tools that look broken against a kernel that's fine.

## Eight relays that did nothing

This is the part worth writing down. With the kernel up and `gpiochip0` exposing
104 lines, driving the relay lines produced exactly nothing. No click. The front
data and link LEDs were equally dead.

Everything about the GPIO layer checked out. The lines existed, the writes
succeeded, the register bits changed.

The relays (GIO88–95), two contacts and the front LEDs share DM355 balls with the
camera and video ports, peripherals this product doesn't use and doesn't populate.
U-Boot leaves those balls muxed to video-in and video-out. On GPIO they therefore
do nothing at all, silently, because the pin isn't connected to the GPIO block in
the first place.

The fix is two register writes, per the DM355 TRM:

| Register | Address | Write | Effect |
|---|---|---|---|
| PINMUX0 | `0x01c40000` | `0x00007955` | GIO86–95 → GPIO (relays 88–95, contacts 86/87) |
| PINMUX1 | `0x01c40004` | `0x0014416A` | GIO75/76 → GPIO (data/link LEDs) |

The PINMUX1 write has to keep bits `[5:0]`, which are PWM0/1/2, the status and
power LEDs, already muxed and driven by U-Boot. Clobbering them trades two working
LEDs for two others.

All eight relays click, and the data and link LEDs light.

There's a nasty companion fact here. DM355 GPIO `IN_DATA` doesn't read back output
pins. A line being driven high reads as 0 on the input register. So there's no
software verification of an output at all: confirming a relay needs ears, and
confirming an LED needs eyes. Every "is it working" question on this board is
answered by a human in the room.

## What's still blocked

The four RS-232 ports and eight IR outputs live behind a Xilinx FPGA on the memory
bus at `0x04000200`, unprogrammed on a netbooted kernel. The serial ports are
stock 16550As at `+0x40/50/60/70` and would work immediately once the FPGA is
loaded.

Loading it's a Xilinx slave-serial bit-bang over seven GPIOs, a standard portable
protocol that can live entirely in userspace with libgpiod. The image is on the
stock NAND. After that, the only genuinely bespoke driver the board needs is
IR-out, which is two small register windows.

The one real gap in Control4's GPL disclosure is here. The drop includes the board
files, the headers and the U-Boot FPGA loader, but leaves out the `c4fpga.c`,
`c4gpio.c` and `c4irout.c` driver bodies. Partial compliance. Not a blocker, since
the digital IO needs no driver and the FPGA loader protocol and register windows
are both documented in the board file that *was* published, but worth recording
plainly.
