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
  four UARTs register as 16550A.
- **Serial I/O confirmed on real RS-232.** With a USB-serial adapter on jack 4,
  bytes sent from the host arrive on `ttyS4` cleanly at **115200** — the UART
  receives and frames real RS-232 data. That also pinned the input clock: the
  data was first garbled, and the vendor's `c4serial.ko` registers these ports
  with `uartclk = 0x02FAF080` = **50 MHz**, not the 27 MHz that was guessed. The
  device tree now carries `clock-frequency = <50000000>`; at 50 MHz a requested
  115200 is divisor 27 → 115740 baud (0.4% off), and the link is clean.
- **ttyS4 proven bidirectional.** An external TX↔RX jumper on jack 4 loops
  `ttyS4` back to itself cleanly (sent `IOX-LOOP-NN...`, read it straight back),
  and the USB adapter self-loops too — so both the IOX TX and RX pins drive and
  read correctly. The one earlier failure (ttyS4 → host reading nothing) was the
  host cable carrying only one direction; a full 3-wire null-modem (cross 2↔3,
  GND 5↔5) fixes it. Nothing in openHC was at fault.
## IR output — driver built & emitting; verification blocked on the emitter
`ohc-iox-irout.c` is now a real **rc-core / lirc TX driver** (8 devices named
`openHC IR out N`, exactly what `iod`'s `lirc.rs` finds — same path as the
HC-800), plus a sysfs calibration harness (`cal_carrier`/`cal_block`/`cal_select`
/`send`/`regs`) at `/sys/.../4000220.irout`. `RC_CORE`/`LIRC` are enabled in the
kernel fragment; it's wired via `objs.mk`.

The full IR protocol was recovered from the vendor `c4irout.ko`/`c4fpga.ko` and
the emit now **completes exactly like the vendor** (CONTROL settles to 0x2200,
GO clears):
- Two 16-bit blocks (0x220/0x230): +0x02 timing (0x017e), +0x04 CONTROL, +0x06
  CARRIER = `(v&0x7f)<<9` (v≤64), +0x08 write-only DATA FIFO, +0x0c COUNT.
- CONTROL = `ENABLE(0x2000) | select` — **no mode bit** (0x4000 wedges it).
- FIFO encoding: mark = `0x8000|d`, space = `d` (no flag), high word (d>0x3fff)
  = `0x4000|(d>>14)`, terminator `0xc000`. Fill at kernel speed.
- Carrier is set by ioctl `_IOW('Z',0x46,u32)` on the vendor; the driver writes
  the reg directly.

**Why it's not yet confirmed radiating:** the GC-IRL learner (on OUT 1) captured
nothing from *my* emit — but it also captured **nothing from the vendor's own
`/dev/irout0`/`irout1`** (carrier set, register-identical), while it learns a
hand-held remote fine. So the FPGA is emitting; the **emitter→learner physical
path** is the blocker (bumped/misaligned/weak emitter after the serial-cable
session, or its jack isn't the one the default select 0x200 drives). This is
hardware, not the driver — the driver matches the vendor register-for-register.

**Morning steps to finish IR:** (1) re-seat/re-aim the IR emitter on OUT 1 over
the GC-IRL window (or point it at a real device / a scope on the jack); (2) with
a working detector, sweep `cal_carrier` via the harness to read the frequency and
pin v→Hz, and sweep `cal_block`/`cal_select` to find OUT 1's routing —
`/private/tmp/.../scratchpad/cal_*.sh` and the vendor `ir_send` tool are ready;
(3) bake the carrier `Hz→v` and the per-jack block/select map into the driver.

## How to load it (dev / netboot)
```
scp fpga_fw.bin root@<box>:/lib/firmware/c4/iox-fpga.bin
echo c4/iox-fpga.bin > /sys/devices/platform/soc/4000200.fpga/firmware
# dmesg → "FPGA CONFIGURED — version 0x0400, DONE=1"; ttyS1-4 appear
```
Or just reboot with the bitstream in place — `S12fpga` does it.
