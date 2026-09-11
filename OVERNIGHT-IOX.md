# IO Extender — overnight report

Short version: **the FPGA is solved on paper.** The one thing that blocked
serial and IR all evening — the pin-mux — is now known from the vendor OS
itself, not guessed. Serial should come up on the next netboot. IR is one
bench calibration away. Own-Verilog is scoped with a clear (hardware) path.

## The breakthrough

We got the stock Control4 OS to boot (unplug Ethernet → power-cycle → `run tst`
fails DHCP → falls through to NAND), where the FPGA is configured and everything
works. From it I read the registers directly with the vendor's own `peeknpoke`:

    PINMUX0 = 0x00000000     <- I had been holding 0x15; the vendor zeroes it
    PINMUX1 = 0x0012416a     <- I had 0x0013416a (bit 16 differs)

`PINMUX0 = 0` is the whole answer for the FPGA config pins. My sweeps missed
M2/M0/DIN because they toggled single bits while holding `0x15`, and each config
pin needs its entire 2-bit field cleared. Zeroing the register clears all four
at once. The live GPIO banks confirmed it: M2/M0 driven high (slave-serial
mode), CCLK an output, **DONE reading high (part configured)**. `S01pinmux` now
writes the vendor values.

## What is done and committed (branch `ioxv1-io`)

- **Pin-mux fixed** to the vendor-authoritative values.
- **Serial map validated**: `/proc/iomem` on the live box shows the four
  `c4serial` UARTs at exactly our DTS addresses (0x240/50/60/70), all four probe
  as 16550A, reg-shift 0 confirmed. Our device tree was right the whole time.
- **NAND, contact polarity, relay-read bug, kexec, armv5te CI** — all fixed
  earlier and holding.
- **FPGA version register** (0x04000200 = 0x0004) — loader now reads it after
  DONE as a second confirmation.
- **Watchdog DT node** added (driver was enabled but never probed — that's why
  the sweep had no safety net the night it hung the box).
- **`--refuse` and `--board` netboot modes** in the flasher; hardcoded addresses
  are now a board fact (ioxv1: serverip 192.168.0.10).

## The vendor drivers, pulled and disassembled

All `c4davinci` modules are `.ko` files on the vendor rootfs — ELF with symbols.
Pulled and disassembled (in scratchpad `c4gpl/mods/`). This recovered:

- **The FPGA loader** (`c4fpga_probe`) — 7 GPIOs, bit-bang, poll DONE. Confirms
  our `ohc-iox-fpga.c` is structurally identical to the vendor's.
- **The IR register map** (`c4irout.ko`) — the file Control4 never published. Two
  blocks (0x20/0x30), eight halfword regs each, CONTROL word bit15=GO. Written up
  in the docs. A **staged IR driver** (`ohc-iox-irout.c`) encodes it and
  registers "openHC IR out N" so iod finds it unchanged — deliberately NOT built
  yet (two calibration constants need the live FPGA; building it now would make
  iod report IR as working when it can't send).

## Do this in the morning

1. Netboot openHC (build is staged at `~/openhc-iox-netboot/openhc-ioxv1-kernel.img`):

       sudo ~/openhc-iox-netboot/ohc-flash netboot --mac 00:0f:ff:18:21:9c \
         --board ioxv1 --image ~/openhc-iox-netboot/openhc-ioxv1-kernel.img --minutes 20

   (`--board ioxv1` supplies the 192.168.0.10/192.168.0.50 addresses. Make sure
   en0 still holds 192.168.0.10 — the tool refuses and tells you if not.)

2. It should boot with `PINMUX0=0`, load `fpga_fw.bin`, DONE should rise, and
   `dmesg | grep ttyS` should show ttyS1-4 as real 16550As. Then the four serial
   ports work through webd/iod exactly like the relays and contacts already do.

3. If serial works, the only remaining item is IR calibration: fire a known
   38 kHz code on the vendor OS, read back the carrier register (0x04000236),
   set the two FIXME constants in `ohc-iox-irout.c`, wire it into `objs.mk`,
   rebuild. That lights up all 8 IR outputs.

## Own-Verilog (the pin map)

Scoped fully in `/iox/fpga-replacement`. The honest verdict: the ball map is
**not** recoverable from software — I confirmed the bitstream is one opaque
168 KB frame block with no open Spartan-3E decoder, and the rootfs has no design
files. BUT the entire `.ucf` *except the ball column* is derivable (it's an EMIF
slave with a known signal list, ~54 nets into VQ100's 66 I/O). The missing
column comes from one JTAG boundary scan via the unpopulated J18 header, or a
self-clocking scanner bitstream (no soldering). Neither is tonight's work, but
both are now concrete rather than vague.

## Live vendor OS

Still up at 10.0.0.113 as of end-of-session (root / t0talc0ntr0l4!, legacy SSH
flags). If it is still there in the morning it is the fastest place to do the IR
carrier calibration before switching back to openHC.
