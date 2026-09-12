# IO Extender — status (daytrip session)

## Working: relays (8), contacts (8), front LEDs, webd, MQTT iod — end-to-end.
## FPGA (serial + IR): still not configuring. But the problem is now pinned down
## HARD, several wrong theories were killed, and there's a clean on-demand
## reference (vendor c4fpga.ko) for the final scope/JTAG step.

---

## The single most important result

I ran the vendor's own driver on demand as a clean A/B:
1. My userspace loader ran on the **vendor OS** (drivers rmmod'd, nothing else
   touching the part) → **FAILED** (blank 0x0202, DONE low).
2. Then `insmod c4fpga.ko` on that same blank part → **CONFIGURED it (0x0400).**

Same board, same kernel, same env, same blank FPGA — vendor driver succeeds,
mine fails. So it is **not the environment and not boot-timing — it is purely a
difference in the load technique/execution.**

## What is DEFINITIVELY ruled out (do not re-chase)

- **Video/VENC clock** — wrong (vendor runs the FPGA with VPSS_CLKCTL=0). Removed.
- **SoC registers** — openHC vs vendor are byte-identical: full system module,
  PLLC1, PLLC2, all 42 PSC MDSTAT modules, PINMUX0-4, async-EMIF CS1 (0x00a00505).
- **A disabled clock** — openHC's clock framework only knows `ref_clk_24m`; it
  disables nothing on the DM355 clock tree (clk_summary confirmed).
- **The driver** — a userspace /dev/mem bit-bang, a direct-`writel` kernel driver,
  and a `gpiod`-DIN kernel driver (exact c4fpga.ko mirror) ALL fail identically.
- **The bitstream** — md5-identical to the vendor's file.
- **PROG polarity/timing** — verified at the pins (PROG high=reset→INIT_B 0,
  low=release→INIT_B 1); 5 ms settle matched.
- **CCLK regularity** — IRQ-off gap-free clocking didn't help.
- **CCLK frequency** — swept 27 kHz … ~6 MHz, all fail.
- **An extra GPIO / clock-enable pin** — live capture during c4fpga.ko's load
  shows it drives only the same 7 config pins, no extra pin.
- **Load-time pinmux** — the mux calls c4fpga.ko makes don't change PINMUX0-4
  from the state my loader already uses (they're effectively no-ops here).

## What's left

The bitstream clocks in cleanly every time (INIT_B stays high = no CRC error),
but DONE never releases (startup never completes). The vendor's exact same
sequence completes startup. Every observable, reproducible difference has been
eliminated, and three independent faithful re-implementations fail identically —
so the remaining difference is at the electrical/pin-timing level, invisible to
register/GPIO polling (too slow to catch the bit-bang) and not present in the
load *logic*.

**Next step = scope / logic analyzer on J18** (or JTAG): capture CCLK, DIN, DONE
and INIT_B during BOTH a vendor `insmod c4fpga.ko` load and an openHC load, and
diff the waveforms. The vendor load is reproducible on demand (see below), so
it's a clean side-by-side. Also worth pulling the actual **c4fpga.c GPL source**
(update.control4.com/open_src) — the kernel loader source is NOT in the patch
set I have (only the U-Boot loader), and a source line could show a detail the
disassembly hid.

## Reproducing / driving it (all set up, no sudo needed after the one supervisor)

- Power-cycle: `~/openhc-iox-netboot/kasa_ctl.py cycle` (Kasa "Bench", local, no creds).
- Netboot target: `echo openhc|vendor|off > /tmp/ohc-flash-ctl/mode`
  (TOGGLE off→openhc to serve a freshly-built image).
- Vendor OS: `mode=vendor` + cycle → 10.0.0.113, root/`t0talc0ntr0l4!`
  (ssh legacy algs). FPGA loader on demand:
  `rmmod`… then `insmod $(find /mnt/jffs2/modules -name c4fpga.ko)` → 0x0400.
- openHC: `mode=openhc` + cycle → 192.168.0.50 (or 10.0.0.115), root/`openhc`.
  Load: scp fpga_fw.bin to /lib/firmware/c4/iox-fpga.bin, `echo c4/iox-fpga.bin
  > /sys/devices/platform/soc/4000200.fpga/firmware`, check `devmem 0x04000200 16`.

## Box state

Left on **openHC** (192.168.0.50), core IO working, FPGA blank. Nothing written
to NAND/bootloader. Commits pushed on `ioxv1-io` (driver + docs). Vendor OS is
one `mode=vendor`+cycle away and its FPGA loader works on demand as reference.
