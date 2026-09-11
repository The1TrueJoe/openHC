---
title: Replacing the FPGA bitstream
description: What it would take to run our own Verilog on the IO Extender's XC3S250E, and why the pin map is the whole problem.
sidebar:
  order: 4
---

The four RS-232 ports and eight IR outputs live inside a **Xilinx Spartan-3E
XC3S250E in a VQ100 package** (`U19`, marked `XC3S250E VQG100BGQ1401`). It is
configured at every boot by the CPU over slave-serial, from
`/control4/lib/fpga/fpga_fw.bin` on the vendor root filesystem.

Running our own design there is attractive for one reason above all: **the
vendor bitstream is Control4's, and openHC cannot ship it.** Every unit has its
own copy in NAND, so extracting it at boot is fine — that is the same position
the IO-MCU firmware is in — but a board whose NAND has been wiped has no route
back, and an openHC bitstream would remove the dependency entirely.

## The part is volatile, so experimenting is safe

Nothing is written to a configuration PROM. The CPU clocks the bitstream in on
every boot, which means:

* loading your own design destroys nothing,
* a power cycle returns the part to blank,
* loading Control4's file restores vendor behaviour exactly.

The only irreplaceable artefact is **a copy of `fpga_fw.bin`**. Take one before
doing anything else.

## The pin map is the whole problem

A `.ucf` needs to say which FPGA ball carries which board net. That information
exists in exactly two places — the board's copper, and the vendor's design
files — and we have neither.

:::danger[It is NOT recoverable from the bitstream]
The configuration image is one opaque block: 42,194 words (168,776 bytes) of
frame data after a short command preamble. IOB configuration *is* in there, but
the frame-to-ball mapping is undocumented for Spartan-3E and there is no open
decoder for it. Project X-Ray covers 7-series; SymbiFlow/F4PGA covers 7-series,
iCE40 and ECP5. Spartan-3E has only partial, abandoned efforts. ISE cannot turn
a `.bit` back into a design either.

Do not spend time trying to decode it. The two routes below are the real ones.
:::

## What we can pin down without any hardware

The FPGA's signal budget, from the vendor board file and the register map. VQ100
gives roughly **66 user I/O**, and the design needs about fifty of them:

| Group | Signals | Notes |
|---|---|---|
| EMIF data | 16 | `D0..D15`, 16-bit bus (the dm9000 on the same CS1 is 16-bit) |
| EMIF address + control | ~10 | enough to decode a 256-byte window at `0x04000200`, plus CS, OE, WE |
| RS-232 | 8 | TX/RX for four ports, to the line transceivers |
| RS-232 flow control | up to 8 | Control4 patched 8250 for AFE on these ports, so RTS/CTS exist |
| IR out | 8 | two 16-byte register blocks at `+0x20` and `+0x30` |
| Interrupt | 1 | to DM355 `GIO2` |
| Clock | 1 | source unidentified |

That is a useful sanity check — the design fits the package with room to spare —
and it tells you how many nets a scan has to resolve. It does **not** tell you
which ball is which.

## Route 1: JTAG boundary scan (recommended)

The XC3S250E implements IEEE 1149.1 and AMD publishes the BSDL file. With a
cheap FT2232 cable and UrJTAG or `xc3sprog`:

* **SAMPLE/PRELOAD** captures the state of every ball while the *vendor*
  bitstream runs and drives real IR and serial traffic.
* **EXTEST** drives each ball individually so you can see which IR emitter
  lights or which RS-232 pin moves.

That produces the complete map from outside the chip, using only public data —
no probing, no continuity testing, no PCB work.

The prerequisite is JTAG access. **`J18`, the unpopulated 2×8 footprint beside
the FPGA, is almost certainly it** — a production programming header meant for
a pogo-pin fixture. It needs a header soldered or a jig.

Identifying the pins costs nothing to get wrong: find GND and 3.3 V with a
meter first (there are `1.2V`, `2.5V`, `1.3V`, `5V` and `GND` test points right
there), then try the rest. A wrong guess on a signal pin fails to read anything;
the confirmation you want is a single number, **`0x01C1A093`**, the XC3S250E's
IDCODE. Read that back and both the header and your wiring are proven at once.

## Route 2: a scanner bitstream (no soldering)

If the header is not an option, the map can be discovered by *observation*
instead — and this needs nothing but the ability to load a bitstream, which is
the same capability the vendor image needs anyway.

Build a design that walks a single `1` across every user I/O in turn, slowly.
Spartan-3E has no internal oscillator, but a **ring oscillator** costs a few
LUTs and needs no external clock, so the design free-runs the moment it is
configured — no EMIF, no clock pin, no chicken-and-egg.

Then watch what responds: an IR receiver identifies the eight emitter balls, a
serial adapter on each rear port identifies the TX pins, and the DM355 can read
its own `GIO2` to catch the interrupt line. Inputs are the harder half and need
a second design that mirrors inputs to an already-identified output.

Slower and more iterative than boundary scan, but it needs no hardware work and
the toolchain is the same either way.

## Toolchain

Spartan-3E means **Xilinx ISE 14.7** — the last version supporting the family.
WebPACK is free and still downloadable, and runs on Linux. The open flows do not
cover this part.

## Do it in this order

1. **Get the vendor bitstream loading first.** It is the reference
   implementation, and until it loads nothing can be compared against anything.
2. **Use it to pin down the register map** — which IR register drives which
   jack, what the UART spacing is.
3. **Then treat that map as the specification** for a register-compatible
   replacement. Done that way the clone needs no driver changes at all: the same
   `ns16550a` nodes and the same IR driver work against either bitstream.
