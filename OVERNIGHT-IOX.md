# IO Extender — status

## Working now, through webd + MQTT iod

- **8 relays** — drivable from the web UI, heard clicking.
- **8 contacts** — all read correctly (contact1's undocumented mux bit solved).
- **Front LEDs** — bound, blink on command.
- **webd + MQTT iod + sysmond** — live; relay toggled browser → webd → iod → MQTT → GPIO.
- **NAND, kexec, contact polarity, relay-read bug, armv5te CI** — all fixed.

## Serial (4) + IR (8): blocked on the FPGA config, and here is exactly where

The four RS-232 ports and eight IR outputs are inside the Spartan-3E, which
boots blank and must be configured each boot. Tonight the pin-mux was solved
(`PINMUX0=0`, read off the vendor OS), so the config pins are finally connected.
The loader now clocks the whole 169 KB bitstream. But the FPGA does not accept
it — its version register stays at the floating `0x0202` instead of `0x0004`.

I matched the vendor in every dimension software can see:

- pins, pin-mux (all 5 PINMUX registers), and the bitstream (md5-identical, intact on the box);
- the bit-bang is **byte-for-byte identical** to the vendor `c4fpga.ko` (verified
  from its disassembly: DIN=MSB, CCLK low, CCLK high).

And it still fails. The catch is that **this DM355's GPIO input registers do not
read back output pins** (a documented quirk), so DONE and INIT_B read
inconsistently and I cannot verify from software whether CCLK/DIN/PROG
physically reach the FPGA — only the FPGA's own version register tells truth,
and it says "not configured." The most likely remaining causes need hardware:

1. openHC's 7.1.8 `gpio-davinci` not physically driving the bank1 (GIO32-63:
   M2/M0/DIN) or bank3 (GIO96/98: CCLK/PROG) output pads, where the vendor's
   2.6.28 driver did. Everything observable matches; the kernel driver is the
   one thing that differs.
2. A config-timing subtlety the slow gpiod toggling violates (less likely —
   slave-serial has no minimum clock).

## Two ways forward — your call

**A. Warm handover (most likely to work, no loader fix needed).** Let the
vendor's *proven* loader configure the FPGA, then reach openHC without a power
cycle or a PROG pulse, so the config SRAM survives. The driver now detects a
pre-configured part via the version register and populates the UARTs without
touching PROG. Sequence:

  1. Boot vendor OS (unplug Ethernet → power-cycle → it falls through to NAND).
     It configures the FPGA; serial + IR work there.
  2. Warm-reboot: from vendor Linux, `reboot` → U-Boot (no power cycle; U-Boot
     does not touch the FPGA, so config is preserved).
  3. Netboot openHC (the same command as always). openHC's probe should see the
     configured FPGA and bring up ttyS1-4.

  Risk: if openHC's kernel drives PROG low during GPIO init before the driver
  runs, it wipes the config. Worth one try — if it works, serial is done.

**B. JTAG / scope.** Put a scope on CCLK/DIN at the FPGA (or the J18 header) to
see whether the pads actually toggle under openHC. That definitively answers
(1). If the pads are dead, it is a kernel gpio-davinci fix; if they toggle, the
bitstream timing is the issue.

## What I would do

Try A first — it is one boot sequence and reuses the vendor's working loader. If
it works, serial and IR are delivered. If not, B pinpoints the kernel-vs-timing
question in minutes with a scope.

Staged build: `~/openhc-iox-netboot/openhc-ioxv1-kernel.img` (sha `157614b5…`),
plus `zImage`/`dm355-hammer.dtb` for kexec. Netboot with
`--board ioxv1`. Box was last at 10.0.0.113 (openHC).
