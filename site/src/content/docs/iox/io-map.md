---
title: IO map
description: The authoritative pin, register and pinmux map for the DM355 "hammer" board.
sidebar:
  order: 2
---

Authoritative map of the on-board IO, from the DM355 technical reference manual
(sprufb3), the vendor `board-hammer.c`, and live bring-up. GIO*n* = DM355 GPIO *n*
= `gpiochip0` line *n* in libgpiod. **All confirmed on hardware unless noted.**

## Pin mux — set this first, or nothing below works

These pins are shared with the (unused) camera and video ports, and **U-Boot
leaves them muxed to video.** On GPIO they therefore do nothing at all, silently,
because the pin isn't connected to the GPIO block.

| Reg | Address | Write | Effect |
|---|---|---|---|
| PINMUX0 | `0x01c40000` | `0x00000155` | GIO82–95 → GPIO (relays 88–95, contacts 3–8) |
| PINMUX1 | `0x01c40004` | `0x0011416A` | GIO70/75/76 → GPIO (contact2, data/link LEDs) |

The field layout is **mainline's own `dm355_pins[]` table** (`arch/arm/mach-davinci/dm355.c`,
last present in v6.1), which is more precise than reading it off the TRM by hand.
Each entry is `MUX_CFG(soc, name, reg, bit, mask, mode)` where `mode` is the value
that selects the **peripheral** — so zero selects GPIO, and every video field here
is two bits wide with `01` = peripheral:

| Reg | Bits | Field | Carries |
|---|---|---|---|
| PINMUX0 | 0–7 | `VIN_CINL_EN` | EMIF address pins — **do not touch** |
| PINMUX0 | 8–9 | `VIN_CINH_EN` | GIO95:94 (relays 7–8) |
| PINMUX0 | 10 | `VIN_YIN_EN` | GIO93:86 (relays 1–6, contacts 7–8) |
| PINMUX0 | 11–14 | `VIN_CAM_HD`, `VIN_CAM_VD`, `VIN_CAM_WEN`, `VIN_PCLK` | GIO82–85 = contacts 3–6 (one bit each) |
| PINMUX1 | 8–15 | `VOUT_COUTH_EN` | includes GIO75/76 (data/link LEDs) |
| PINMUX1 | 16 | `VOUT_HVSYNC` | unidentified |
| PINMUX1 | 18–19 | `VOUT_FIELD` | GIO70 = **contact2** |

:::caution[Keep PINMUX1 bits [5:0]]
Those are PWM0/1/2, the status and power LEDs, already muxed and driven by
U-Boot. Clobbering them trades two working LEDs for two others.
:::

`board/ioxv1/rootfs-overlay/etc/init.d/S01pinmux` applies these at boot. That's a
runtime stand-in; the proper fix is DT pinctrl.

**Confirmed on hardware:** the relays and the data/link LEDs.
**Not yet confirmed:** the contact bits above. They follow the same field layout
and the same reasoning, but nothing has read a contact through them yet.

:::caution[contact1 = GIO71 is still unresolved]
Mainline names PINMUX1 bits 18–19 `VOUT_FIELD`, with an alias `VOUT_FIELD_G70`
for mode 0 — **GIO70**, i.e. contact **2**. No field anywhere in `dm355_pins[]`
is identified as GIO71, so contact 1 has no known mux bit.

Settle it the way the relays were settled: hold a closure on terminal 1 and sweep
one PINMUX1 field at a time, watching `gpioget contact1`. Do not guess a value in
`S01pinmux` — PINMUX0 bits 0–7 are the EMIF address pins, and a wrong write there
takes out NAND, the dm9000 and the FPGA at once.
:::

## Relays (8) — confirmed clicking

`gpiochip0` lines **88–95** (relay1..relay8), active high (1 = energized). Direct
SoC GPIO, no driver needed.

## Contacts (8, inputs)

`gpiochip0` lines 70, 71, 82, 83, 84, 85, 86, 87. Lines 86/87 are already GPIO
after the PINMUX0 fix; 70/71 and 82–85 still need their pinmux.

## LEDs

| Name | Type | How to drive | Notes |
|---|---|---|---|
| **data** (front) | GPIO | `gpiochip0` line **75**, active high | 1 = on |
| **link** (front) | GPIO | `gpiochip0` line **76**, active high | 1 = on |
| **status** (front) | PWM0 @ `0x01c22000` | orange | blink = slow period; steady = small PER |
| **power** (front) | PWM1 @ `0x01c22400` | blue | |
| (red) | PWM2 @ `0x01c22800` | red | part of the tri-colour status |
| rear status/data/link/power | **TBD** | — | not in the vendor board file; needs discovery |

PWM registers, base + channel × `0x400`: PCR `+0x04`, CFG `+0x08`, START `+0x0c`,
RPT `+0x10`, PER `+0x14` (period), PH1D `+0x18` (phase-1 / duty). The LEDs are
**active low**. A ~1 s PER reads as a blink; a small PER with PH1D ≈ PER/2 is a
steady glow.

The status and power LEDs are already muxed and driven by U-Boot, so they are
controllable through those PWM registers with no pinmux work.

## GPIO registers for `devmem`

Base `0x01c67000`. The per-bank block is at `0x10 + (gpio/32) * 0x28`:

| Offset | Register |
|---|---|
| `+0x00` | DIR (1 = in, 0 = out) |
| `+0x04` | OUT |
| `+0x08` | SET |
| `+0x0c` | CLR |
| `+0x10` | IN |

with bit = `gpio % 32`. So the bank block for GIO64–95 is at `0x01c67060`, and the
relays are bits 24–31.

:::danger[There's no software verification of an output]
**DM355 GPIO `IN_DATA` does not read back output pins.** A line being driven high
reads as 0 on the input register — GPIO 101 also reads IN = 0 while driven. So
confirming a relay needs ears and confirming an LED needs eyes. Every "is it
working" question on this board is answered by a human in the room.

`tools/ioxv1-iotest.sh` is the harness for walking through them.
:::

## Ethernet — working

`dm9000` → `eth0`. IRQ = GIO1 (rising edge), reset = GIO101, MAC forced to
`00:0f:ff:18:21:9c` because the part has no EEPROM.

## Serial

| Port | Device | Backing | Status |
|---|---|---|---|
| debug console | `ttyS0` | SoC UART0 @ `0x01c20000` | working (RX only unless TX is wired) |
| RS232 1–4 | `ttyS1-4` | **FPGA UARTs** @ `0x04000240/250/260/270` | **blocked: FPGA not programmed** |

The FPGA UARTs are stock 16550As and need only `ns16550a` DT nodes on a shared
IRQ (GIO7) once the FPGA is loaded — but **their register geometry is not the SoC
UART's.** Each window is 16 bytes for 16 registers, so `reg-shift = <0>` and
`reg-io-width = <1>`; `ttyS0` at `0x01c20000` is `reg-shift = <2>`. Getting that
wrong gives you a port that probes and then talks to the wrong registers.

They also do **hardware flow control**, which is not obvious from the part they
claim to be. Control4 patched `8250.c` with a `UPF_C4_SUPPORT_AFE` flag purely for
these, their comment explaining it exactly: *"the fpga supports flow control but is
detected as a 16550A"*. Mainline has no equivalent flag, so `CRTSCTS` on these
ports needs that patch forward-ported — RTS/CTS is not needed for a console, but
is for anything driving a projector that asserts it.

Their input clock is still unknown. The vendor's SoC ports run at 27 MHz but these
are clocked inside the FPGA and no GPL source states the rate. `setserial -a
/dev/ttyS1` on a vendor unit reports `baud_base`, which is `uartclk / 16` — that
is the cheapest way to settle it. The DT currently carries 27 MHz as a **flagged
placeholder**.

**CE1 (`0x04000000`+) isn't readable from userspace `devmem`.** It needs a
kernel driver, or at least AEMIF CS1 timing configured, before the window responds.

## IR-out (8) — FPGA

FPGA at `0x04000220` and `0x04000230`. Also needs the FPGA programmed, and then a
small bespoke driver whose register semantics come from the on-device
`c4irout.ko`.

## Buttons

ID button on GIO9, recovery button on GIO8.

## FPGA slave-serial pins

For the Xilinx bit-bang loader:

| Signal | GPIO |
|---|---|
| M2 | 55 |
| M0 | 57 |
| CCLK | 96 |
| DONE | 97 |
| DIN | 58 |
| INIT_B | 7 |
| PROG_B | 98 |

Standard protocol: pulse PROG_B, wait for INIT_B, clock each bit on DIN/CCLK,
wait for DONE. It can be a tiny kernel driver or a userspace libgpiod tool; the
image `fpga_fw.bin` is on the stock NAND.

**The loader does not have to be written from scratch — Control4 published it.**
`cmd_c4fpga.c` and `c4fpgaldr.c` are in `u-boot-1.2.0.tgz+patches.tar.gz`
(`patches/uboot-video-fpga.patch`), GPL, complete. That copy is for a different
board so its GPIO numbers are another product's, but the sequence is the whole
answer: drive M2/M0 high, PROG_B high → 20 µs → low, 200 µs settle, check INIT_B
is high, then per byte send bits **MSB first** — DIN, CCLK low, CCLK high — and
when the bitstream is done clock **12 dummy cycles** and read DONE.

:::caution[INIT_B is GIO7, and so is the UART interrupt]
The same line is a programming status pin during the load and the shared serial
interrupt afterwards. A loader that keeps hold of it leaves the four UARTs with no
usable IRQ.
:::

## I²C

`davinci-i2c` at 400 kHz, bus 1: a 24c08 EEPROM at `0x50`. The temperature sensor
is behind the FPGA.

## Where the vendor source is — and what is missing from it

Control4's GPL drops are still live, old-scheme, on their CDN:

```
http://update.control4.com/open_src/<VERSION>-res/src/<name>+patches.tar.gz
```

Directory listing is refused, but `src-<VERSION>.xml` indexes every file with its
size and MD5. `2.9.0.525559-res` and `2.10.0.540110-res` both resolve and carry
identical DM355 sources. The two that matter:

| Package | What is in it |
|---|---|
| `linux-davinci-2.6.28-rc8+patches.tar.gz` | `patches/2.6.32/arch-arm-mach-davinci-board-hammer_c.patch` — **the board file**, and the source of nearly every number on this page |
| `u-boot-1.2.0.tgz+patches.tar.gz` | `patches/uboot-video-fpga.patch` — the complete Xilinx slave-serial loader |

`board-hammer.c` gives, verbatim: relays `GIO88–95` active-high named `relay1..8`;
contacts named `contact1..8` on `GIO71, 70, 82, 83, 84, 85, 86, 87` (note the
reversal at the front); `c4serial_fpga_resource` = the four UART windows plus
`IORESOURCE_IRQ` on `GIO(7)`; `c4irout_fpga_resource` = the two IR windows;
`c4fpga_desc.ss` = the seven programming pins; `id-btn` on `GIO9` and
`recovery-btn` on `GIO8`; the PWM LED names; and the full NAND partition table.

:::danger[The IO drivers themselves are NOT in the drop]
`c4fpga-drivers.patch` adds `drivers/control4/Kconfig` and `Makefile` — which name
`c4fpga.o c4gpio.o c4irout.o c4serial.o` — and **no `.c` files for any of them**.
Neither kernel package contains `drivers/control4/*.c`, and neither does
`all_current_patches.tar.gz` (that one is MontaVista's, not Control4's).

So the FPGA register semantics for IR and the UART clock cannot be read out of the
published source. They have to come off a unit: `c4irout.ko` and `c4serial.ko` on
the stock NAND rootfs, disassembled. Do not spend another evening searching the
CDN for them — this page is the record that they are not there.
:::
