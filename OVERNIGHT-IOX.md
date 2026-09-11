# IO Extender — status

## Working now (webd + MQTT iod): 8 relays, 8 contacts, front LEDs, all end-to-end.

## FPGA (serial + IR): root cause CONFIRMED and the fix is WRITTEN. Needs a build + a hardware test.

This session closed the loop on *why* the FPGA loaded cleanly but never
finished starting up — and it was provable **offline**, from the vendor
bitstream and vendor GPL source, without the board.

### The chain, start to finish

1. **The data path was already perfect** (last session): the whole 169 KB
   bitstream clocks in with INIT_B high the entire time — zero CRC errors.
2. **But DONE never released** and the FPGA register bus stayed floating
   (version reg 0x0202 instead of 0x0004). Extra start-up clocks did nothing.
3. **Why (confirmed by parsing the bitstream):** the vendor bitstream's COR
   register is `0x000031e5`. Decoded, its **LCK_cycle field = 0**, i.e.
   *wait for DCM lock at start-up phase C0* — versus the bitgen "NoWait"
   default of `0x3fe5`. The only bits that differ are exactly the LCK_cycle
   field (`0x0e00`). So the part's start-up state machine **parks at C0 until
   its DCM locks**; DONE is scheduled for C7 and is never reached. No number of
   CCLKs can move it — it's waiting on a *clock*, not on clocks-we-send.
   (Parser: `/private/tmp/.../c4gpl/wk/parse_bitstream.py`.)
4. **Which clock (confirmed from vendor GPL U-Boot):** the DM355 **video
   encoder's digital LCD clock (VENC DCLK)**. The IOX has no display, but its
   VENC is wired to the FPGA purely as a clock source. The vendor turns it on
   *in U-Boot* before loading the part (`uboot-video-fpga.patch`:
   `c4fpgaldr.c` + `davincifb.c` `enableDigitalOutput()`), then boots Linux
   with the part already configured and the clock already running.
5. **Why openHC misses it:** we netboot straight to `bootm` and load the FPGA
   from the *kernel*, long after (and instead of) that U-Boot video init. The
   clock is never started, the DCM never locks, DONE never releases.

### The fix (committed, in `ohc-iox-fpga.c`)

The kernel FPGA loader now starts the VENC DCLK itself, at probe, before
loading — using the exact register set and values from the vendor U-Boot:

- Power the **VPSS master/slave** PSC modules (LPSC 0 and 1, PSC @ 0x01c41000).
- `VPSS_CLKCTL` (**0x01c40044**) = `0x0a`  ← the real VPSS clock control; note
  this is *not* 0x01c70200, which earlier notes had wrong.
- VENC: `VPBE_PCR`=0, `VIDCTL`=VLCKE(bit13), `DCLKCTL`=DCKEC(bit11), and the
  DCLK pattern (`DCLKPTN0`=1, `DCLKPTN0A`=2, `DCLKHSA`=1), plus OSDCLK.

openHC runs no davinci PSC clock driver, so nothing gates the domain back off —
the clock just keeps running, which is what the FPGA's UART logic needs anyway.

### To test (needs the user — hardware access)

1. Push this branch; dispatch the ioxv1 image build in CI.
2. Netboot the box with the new image (`ohc-flash netboot` **must run as root** —
   TFTP binds port 69; that blocked the automated attempt this session).
3. `dmesg | grep -i fpga` — expect "FPGA clock (VENC DCLK) enabled …" then,
   after writing the firmware, **"FPGA CONFIGURED — version 0x0004"** and the
   four `ttyS1..4` plus the IR block appearing.

If DONE still doesn't release, the DCLK pattern/divider is the only remaining
knob — the vendor's exact pattern is used, but the VENC pixel-clock source
(VPSS_CLKCTL value / PLL) is the thing to scope next.

### Note on the box right now

It is sitting in a U-Boot **netboot retry loop** ("Retry count exceeded;
starting again") — it never fell through to NAND. Ctrl-C over the console does
not break this U-Boot's TFTP loop, and serving it an image needs root (port 69),
so it needs either a power-cycle or a root `ohc-flash netboot` to move. Nothing
was written to NAND or the bootloader.

## Bottom line

The FPGA mystery is solved end to end and the fix is in the tree. What's left is
mechanical: build it, netboot it as root, and confirm DONE releases. Then serial
(ttyS1-4) is free and the staged IR driver can be wired up.

Staged build inputs: `~/openhc-iox-netboot/`. Bitstream + vendor GPL source:
`/private/tmp/.../c4gpl/` (bitstream `nand/fpga_fw.bin`, U-Boot loader
`patches/uboot-video-fpga.patch`).
