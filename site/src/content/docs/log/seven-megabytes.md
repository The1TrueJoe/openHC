---
title: Seven megabytes
topic: Kernel
summary: The bootloader copies your kernel over its own loader if the kernel is too big. Plus a console running at eight times the requested baud, and PCI devices with no addresses.
description: The CEFDK copy window, the kernel size budget, the 8x UART clock, and unassigned PCI BARs.
sidebar:
  order: 4
  label: Seven megabytes
---

With an unlocked shell and an unverified boot command, the rest looked like
packaging. Build a bzImage, stage it, jump to it. The first builds hung without a
word on the console.

## The ceiling nobody mentions

`bootlinux` copies the protected-mode kernel to `0x100000`. Its own loader code
lives at about `0x812f07`, with the final jump near `0x8130be`.

If the copy reaches that, the kernel overwrites the loader while the loader is
running, and it crashes on return. You get no diagnostic, because the code that
would print one has just been replaced by your kernel.

So the protected-mode kernel has to stay under roughly 7.0 MB
(`0x813000 − 0x100000`). That one number governs every config decision on the EA
family, and it isn't documented anywhere. It falls out of reading where CEFDK put
itself.

The stock x86 `i386_defconfig` is a "support every PC ever built" config. A
kernel-only image busts that budget on its own, before any initramfs.

## Getting under it

Keep the initramfs separate rather than embedded, then trim hard:

- `CONFIG_KERNEL_XZ` and `CONFIG_CC_OPTIMIZE_FOR_SIZE`
- `CONFIG_MODULES=n`, everything built in and self-contained. This also sidesteps
  an i386/7.1.8 modpost failure where the musl toolchain compiles `.ko` files with
  the stack protector but `__stack_chk_guard` isn't exported to modules.
- `CONFIG_STACKPROTECTOR=n`, or the toolchain's canary disagrees with i386 SMP
  setup and `__stack_chk_fail` panics in `do_idle` on the secondary CPU. That one
  presents as a mysterious second-CPU panic and is entirely a toolchain
  disagreement.
- `CONFIG_KALLSYMS=n`, and drop the VM-guest, debug, storage, graphics, EFI,
  netfilter and IPv6 dead weight.

Then force what's actually needed to `=y`: ath9k, e1000, cfg80211, mac80211, 8250.

Result: Linux 7.1.8 in 4.82 MB, about 2.2 MB under the ceiling.

That headroom has since been spent, on purpose and with the numbers written down.
Building the board x86_64 costs about 7 MB on its own. Adding the
[SGX driver](/log/the-impossible-gpu/) costs another 1.25 MB, most of which
is DRM core rather than the driver itself. The
[size budget](/ea/boot-chain/#the-copy-window) is now the first thing any
new feature gets weighed against, and it's why several features currently ship
switched off.

## The console lies about its speed

The kernel booted and printed garbage. Not nothing. Garbage, which is a different
and much more useful symptom.

The CE5310's legacy `0x3f8` UART clock is eight times standard, 14.7456 MHz. A
mainline kernel asked for 115200 therefore drives the wire at 921600. CEFDK knows
the real clock and prints correctly at 115200; the kernel doesn't, and nothing
warns you.

Read the kernel console at 921600. Output is clean, typing back is overrun-prone.
The proper fix is setting the legacy `uartclk`, or the `console=ttyS0,14400`
trick, where divisor 8 gives a true 115200 on the wire.

Two hours of "the kernel is crashing very early" was a terminal at the wrong
speed.

## PCI devices with no addresses

Next symptom. The kernel booted properly, found `ath9k` and `e1000`, and neither
worked. No `wlan0`, no `eth0`.

CEFDK only programs the BARs for devices it uses itself. Everything else comes up
with BAR 0 = `0x00000000`, and the kernel can't `ioremap` an address of zero.

One boot argument fixes it: `pci=realloc,nocrs`, which tells the kernel to assign
the BARs itself instead of trusting the firmware's map. It's baked into
`CONFIG_CMDLINE` with `CMDLINE_EXTEND` so it can't be forgotten.

Both of these are the same category of problem, and it's the characteristic one on
this platform. The firmware did a partial job and didn't say so. The UART clock is
non-standard and unannounced, the PCI config is incomplete and unannounced.
Neither is a Linux bug and neither produces an error message. You find them by not
believing the machine.

## What the tree looks like because of it

The size constraint is also why the EA boards are composed rather than copied.
Buildroot has no include mechanism for defconfigs, so the build script
concatenates them: a common file for every board, then the EA family base, then
whichever feature sets the board lists in `ohc.features`, then the board's own
file. Kconfig takes the last assignment, so a board extends or overrides shared
settings just by coming later.

In practice that means a feature that costs kernel size is a line in a text file,
and turning it off to make room is also a line in a text file. With a 7 MB ceiling
and a 5% headroom margin, that flexibility isn't a nicety.

There's a standing note in the repo listing what's currently switched off to keep
an image under the window and exactly why, with the measured byte counts. It reads
like an admission of defeat and it's the opposite. The numbers are known, so the
trade is a decision instead of a surprise.

## Proven, and then the floor moved

At this point an EA1 netbooted a modern kernel, came up on Wi-Fi and ran SSH. No
fuse blown, no password cracked, no SPI programmer, no flash written, and the
factory restore button still worked.

Then I tried the same image on an EA3.
