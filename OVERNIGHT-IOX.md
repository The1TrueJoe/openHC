# IO Extender — status: FPGA SOLVED ✅

## Working end-to-end
- Relays (8), contacts (8), front LEDs, buttons — native SoC GPIO.
- webd, MQTT iod.
- **FPGA configures under openHC** — version register reads `0x0400`, DONE high,
  stable. This was the last blocker for the board.
- **All four FPGA UARTs (ttyS1–ttyS4) probe as 16550A** and are register-perfect
  (see "confirmed vs pending" below).

## The breakthrough — two root causes, both fixed

For the whole project the FPGA "clocked in clean" (INIT_B stayed high) but never
configured, and it was wrongly concluded to need a scope/JTAG. The real problem
was in software the whole time:

1. **The config pins are multiplexed with EMIF address lines** (this is the one
   the vendor does and openHC didn't). PINMUX2 is the DM355 "Pin Mux 2 (AEMIF)"
   register (SPRUFB3 Table 9-6); its low bits select EMIF address vs GPIO,
   polarity `0 = EMIF, 1 = GPIO`:
   - bit 0 `EM_A13_3` → `GIO[64:57]` — **m0 (GIO57), din (GIO58)**
   - bit 1 `EM_A0_BA1` → `GIO[56:55]` — **m2 (GIO55)**

   So `m2/m0/din` ride on EMIF address pins. Until `PINMUX2[0:1] = 1` those pins
   are address lines, not GPIO — **DIN never reaches the FPGA**, which is exactly
   why it clocked "clean" (INIT_B high) yet never synced: the data line was
   disconnected. The vendor `c4fpga.ko` sets both bits for the load
   (`dm355_mux_peripheral(8,0,1)/(8,1,1)`) and clears them after, so the same
   pins revert to `EM_A[]` to *address* the configured FPGA's registers. openHC
   only ever held the post-load value. **Fixed:** the driver now sets
   `PINMUX2 |= 0x3` for the load and restores it.

2. **The version register must be read after restoring PINMUX2.** The version at
   CS1+0x200 needs `EM_A[8]`, which is muxed away while the config pins are GPIO;
   reading it mid-load returned `0x0000` and masked the success. **Fixed:** read
   after restore. (DONE is read via gpiod and is pinmux-independent — it is the
   reliable "configured" signal, and it reads high.)

3. **The FPGA 16550 UARTs are `reg-shift 1`, not 0.** The 8-bit registers sit in
   the low byte of each 16-bit EMIF word, two bytes apart. Confirmed on the
   configured part: at `reg<<1`, IIR=`0xc1`, LSR=`0x60`, and DLL/DLM + the
   scratch register hold writes; at reg-shift 0 the scratch test reads back 0
   (which is why the ports "detected" but could not do I/O). **Fixed** in the DTS.

## In the repo now (branch `ioxv1-io`)
- `board/ioxv1/kernel/drivers/misc/ohc-iox-fpga.c` — PINMUX2 set/restore during
  load; version read after restore; success judged by DONE + version.
- `board/ioxv1/patches/linux/0004-…hammer.patch` — UART nodes `reg-shift = <1>`.
- `board/ioxv1/rootfs-overlay/etc/init.d/S12fpga` — triggers the load at boot.
- `board/ioxv1/firmware/fpga/README.md` + `rootfs-overlay/lib/firmware/c4/` — the
  proprietary bitstream is git-ignored (owner supplies it); drop `iox-fpga.bin`
  in the overlay dir to bake it in, or scp it to `/lib/firmware/c4/`.

## Confirmed vs pending
- **Confirmed:** FPGA configures every load; driver reports `FPGA CONFIGURED —
  version 0x0400, DONE=1`; `of_platform_populate` brings up the children; all
  four UARTs register as 16550A; their divisor latches and status registers
  read/write correctly at reg-shift 1 (LSR=0x60 idle on all four).
- **Pending (needs physical access):** actual byte-level RS-232 TX/RX over the
  jacks (internal MCR loopback isn't wired in the FPGA core, so a cable/scope is
  needed to see bytes leave), and the true UART input clock — `clock-frequency`
  is a 27 MHz placeholder; read `setserial -a /dev/ttyS1` baud_base off a vendor
  unit for the real rate. IR (`ohc-iox-irout.c`) is staged but not built:
  carrier calibration + jack map still need the live part.

## How to load it (dev / netboot)
```
scp fpga_fw.bin root@<box>:/lib/firmware/c4/iox-fpga.bin
echo c4/iox-fpga.bin > /sys/devices/platform/soc/4000200.fpga/firmware
# dmesg → "FPGA CONFIGURED — version 0x0400, DONE=1"; ttyS1-4 appear
```
Or just reboot with the bitstream in place — `S12fpga` does it.
