# IO Extender — status

## Working now (webd + MQTT iod): 8 relays, 8 contacts, front LEDs, all end-to-end.

## FPGA (serial + IR): the bitstream now LOADS cleanly. One step left.

Huge progress this session. The FPGA now accepts the entire vendor bitstream
with **zero CRC errors** — the hard part is solved. The remaining blocker is
narrow and well-characterised.

### The bugs found and fixed (all real, all committed)

1. **Pin-mux** — `PINMUX0=0` (read off the vendor OS). Connects every config pin.
2. **INIT_B gating** — removed; INIT_B on GIO7 reads via `gpioget` but was
   mis-read by raw devmem (GIO0-9 are unbanked).
3. **PROG polarity was INVERTED** — the real killer. On this board, driving
   GIO98 **low RELEASES** the FPGA and **high holds it in reset** — backwards
   from a textbook PROG_B. The DTS said `ACTIVE_LOW`, so the loader's pulse
   ended with the part *held in reset*, clocking 169 KB into a chip that
   couldn't listen. Now `ACTIVE_HIGH`.
4. **gpiod vs direct registers** — the gpiod path didn't reproduce the proven
   raw sequence, so the whole config-pin path (M2/M0/DIN/CCLK/PROG) is now
   direct GPIO register writes, byte-identical to the sequence verified on
   hardware.

### Proof it's working

After a full load: **INIT_B = 1** — the FPGA accepted all 169 KB with no CRC
error. The data path (DIN/CCLK/PROG) is 100% correct. Verified independently by
clocking the 128-byte header raw and watching INIT_B stay high through the
IDCODE check.

### The one remaining thing: DONE won't release / startup won't complete

INIT_B high (data good) but DONE stays low and the FPGA's register bus never
activates (version reg floats at 0x0202 instead of 0x0004). Extra startup
clocks (256+ over ~0.5s) don't release it. DONE is correctly a floating input
(not driven low by the SoC). So the config data is accepted but the **startup
sequence isn't completing**.

**Leading hypothesis — a missing functional clock, tied to "no video":**
the FPGA likely needs a clock (the ~27 MHz used by the DM355 video/VPBE path)
for its DCM to lock and startup to finish. openHC has that subsystem stripped:
`VPSSCLKCTL (0x01c70200) = 0` and the VPSS PSC modules are disabled. The vendor
kernel runs video, so the clock is present there and the FPGA configures. Same
board, same bitstream — the difference is that clock.

### How to settle it (next session)

1. **Vendor-OS clock dump (cleanest, low risk).** Boot the vendor OS (unplug
   Ethernet → power-cycle → NAND), where the FPGA works, and read
   `VPSSCLKCTL`, the PSC MDSTAT states, and the PLL config. Diff against
   openHC's (captured: VPSSCLKCTL=0, VPSS modules off). Whatever the vendor has
   enabled that openHC doesn't is the FPGA's clock — then enable it in openHC.
2. **Scope / JTAG.** Confirm whether a clock is present at the FPGA, and watch
   DONE. Definitive.

I did NOT blind-poke the PLL/VPSS registers — a wrong write there can hang the
SoC, and the payoff needs the vendor comparison to aim it correctly.

## Bottom line

We went from "nothing" to "the FPGA accepts the entire bitstream cleanly." Only
the startup clock stands between here and live serial + IR. The IR driver is
already written from the recovered register map; serial needs no new code.

Staged build: `~/openhc-iox-netboot/` (kernel + zImage + dtb). Box last at
10.0.0.113. Bitstream at `/private/tmp/.../c4gpl/nand/fpga_fw.bin`.
