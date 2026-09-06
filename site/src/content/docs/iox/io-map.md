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
| PINMUX0 | `0x01c40000` | `0x00007955` | GIO86–95 → GPIO (relays 88–95, contacts 86/87) |
| PINMUX1 | `0x01c40004` | `0x0014416A` | GIO75/76 → GPIO (data/link LEDs) |

Per the TRM's System Module section: PINMUX0 bit 10 is `YIN` = GIO[93:86] and bit
9 is `CIN` = GIO[95:94], with `0` meaning GPIO. PINMUX1 bits `[13:12]` are `COUT1`
= GIO75 and `[11:10]` are `COUT2` = GIO76.

:::caution[Keep PINMUX1 bits [5:0]]
Those are PWM0/1/2, the status and power LEDs, already muxed and driven by
U-Boot. Clobbering them trades two working LEDs for two others.
:::

`board/ioxv1/rootfs-overlay/etc/init.d/S01pinmux` applies these at boot. That's a
runtime stand-in; the proper fix is DT pinctrl.

**Still to do, same mechanism:** contacts GIO70/71 (PINMUX1 bits 17–19) and
GIO82–85 (PINMUX0 bits 11–14).

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
IRQ (GIO7) once the FPGA is loaded. Their input clock frequency is still unknown —
recover it from `c4serial.ko` or measure it.

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

## I²C

`davinci-i2c` at 400 kHz, bus 1: a 24c08 EEPROM at `0x50`. The temperature sensor
is behind the FPGA.
