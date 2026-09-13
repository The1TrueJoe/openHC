# fpga — the IO Extender's Xilinx bitstream

The four RS-232 ports (`ttyS1`–`ttyS4`) and eight IR outputs on the IO Extender
are registers inside a **Xilinx Spartan-3E XC3S250E** (`U19`) on async EMIF CS1.
It powers up **blank**: until a configuration bitstream is loaded, those
registers do not answer and no `ttyS1`–`ttyS4` exist. Relays, contacts, front
LEDs and buttons are native SoC GPIO and work with no bitstream at all.

The bitstream is **proprietary Control4 firmware and is not in this repo** — the
same policy as the IO-MCU images (`*.bin` is git-ignored; the owner of the
hardware already has the file). It is `fpga_fw.bin` on a stock unit, **169,216
bytes**, IDCODE `0x01C1A093`.

## Getting the file off a vendor unit

It ships on the Control4 rootfs at:

```
/control4/lib/fpga/fpga_fw.bin
```

Copy it off a running vendor OS (or a NAND dump). That one file is all that is
needed; openHC loads it exactly as the vendor's `c4fpga.ko` does.

## Installing it

Name it `iox-fpga.bin` and put it in **either** place:

1. **Baked into the image** (recommended for a standalone box) — drop it at
   `board/ioxv1/rootfs-overlay/lib/firmware/c4/iox-fpga.bin` before building.
   The overlay copies it to `/lib/firmware/c4/iox-fpga.bin`; it stays git-ignored.

2. **On a running box** — `scp` it to `/lib/firmware/c4/iox-fpga.bin`, then run
   `/etc/init.d/S12fpga start` (or reboot).

Either way, `S12fpga` triggers the in-kernel loader at boot and the ports come
up. If the file is absent the board still boots — just with no serial or IR, and
a clear log line saying so.

## How the load works (for reference)

`ohc-iox-fpga` (built in, `board/ioxv1/kernel/drivers/misc/ohc-iox-fpga.c`)
bit-bangs the stock Xilinx slave-serial sequence over GPIO
(`m2=55 m0=57 din=58 cclk=96 done=97 prog=98 initb=7`). The one non-obvious
step, and the reason a naive port never worked: **`m2`/`m0`/`din` share pins
with EMIF address lines**, so the driver sets `PINMUX2[0:1]` to route them to
GPIO for the load and restores them afterwards, before reading the FPGA version
register at CS1+0x200 (a configured part reads `0x0400`, DONE high). See the
driver's PINMUX2 comment and `[[c4-iox-fpga]]` for the full story.
