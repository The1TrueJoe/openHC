---
title: The MCU that would not answer
topic: IO
summary: A microcontroller that answered nothing looked like dead hardware for days. Its firmware contains the wrong baud rate nineteen times, and that rate belongs to a different port.
description: Bringing up the EA family's IO microcontroller, decoding its profile table, and confirming IR in both directions.
sidebar:
  order: 6
  label: The MCU that would not answer
---

The EA family's IR jacks, relays, contacts and combo serial ports don't hang off
the SoC. They hang off a TI Tiva **TM4C1231D5**, a Cortex-M4F with 64 KB of flash,
reached over a host UART. From Linux's point of view there's nothing there: no
`/sys/class` entries, no device nodes, no GPIO lines. Just a serial port and a
protocol nobody documented.

Probing that port produced silence. Not garbage at the wrong rate. Silence, which
is exactly what a dead or unpopulated part looks like.

## The trap

The application image contains the constant `115200` nineteen times, sitting
immediately after the UART0/UART5/UART7 base addresses. That reads like proof.

It isn't. Those are the bauds of the MCU's *own* user serial ports, the combo
jacks on the back panel. The host link on UART0 runs at 460800.

What makes this cost days rather than minutes is the failure mode. The bootloader
autobauds. It syncs to whatever speed the host uses, so a host at the wrong speed
gets silence instead of garbage at some other rate, and silence is
indistinguishable from absent hardware. Every diagnostic instinct points at the
wiring.

The vendor's own daemon says so plainly once you turn its logging up:

```
Opening (/dev/ttySIO).
Set UART Speed (115200).        <- initial open only
STATE_MACHINE :: Set State from Unknown to bootloader
Set UART Speed (460800).        <- and everything after this is 460800
```

It opens at 115200, immediately renegotiates, and never uses 115200 again.

## And it boots into its bootloader

Second half of the same problem: the MCU comes up in the TI serial bootloader,
not in the application. It has to be told to run.

The sequence, which is plain TI bootloader protocol:

| Step | Bytes out | Reply |
|---|---|---|
| 1. pulse `io_reset` low→high | — | MCU enters bootloader |
| 2. autobaud | `55 55` | `00 cc` (ACK) |
| 3. PING | `03 20 20` | `00 cc` |
| 4. GET_STATUS *(optional)* | `03 23 23` | `00 cc` + `03 40 40` |
| 5. RUN app at 0x1000 | `07 32 22 00 00 10 00` | **none, by design** |
| 6. autobaud the app | `55 55` | `10 02 d7 ...` |

Two details produced their own dead ends.

`COMMAND_RUN`'s address is big-endian and never acknowledged. The vendor's own
flasher passes `0` for the wait-for-ack argument on this command and `1` on every
other one, because the MCU has already jumped by the time an ACK could be sent.
Waiting for one just times out.

And the host must ACK any packet the MCU sends. Leave a `GET_STATUS` reply
unacknowledged and the bootloader desyncs, after which every subsequent command is
silently ignored. That's what made an early big-endian RUN attempt look like a
byte-order bug when it was a desync three steps earlier.

## What the firmware actually implements

With the link up, a sweep of all 256 opcodes against live application 1.0.36 found
only these answering:

| Request | Reply | Payload | Meaning |
|---|---|---|---|
| `0x24` | `0x25` | `c4:io_processor:c4-ir02` | product name |
| `0x34` | `0x35` | `1.0.36` | firmware version |
| `0x54` | `0x55` | `00 00` | relay state |
| `0x74` | `0x75` | `00 00 00 00` | contact state, a u32 bitmask |
| `0xa1` | `0xa4` | `01` | UART send → ready for data |
| `0xd2` | `0xd7` | `00 07 <u16>` | autobaud measurement |

`CAPABILITIES_GET` isn't implemented, which matters because the obvious way to
write a host driver is to ask the device what it has. The product string
`c4-ir02` fits, a minimal IR-focused build. So the host treats a capabilities
timeout as normal rather than an error.

The startup frames are a free hardware census. The app sends one unsolicited
`UART_RECEIVE` per user serial port, so an EA1 announces exactly two:

```
10 02 a2 00 00 00 02 00 00 5c     port 0
10 02 a2 01 00 00 02 01 00 5a     port 1
```

An EA3 sends three. It's the cleanest non-invasive way to count user serial ports
on any of these boards.

## One image, six boards

`.flash.config` maps ea1, ea3 and ea5 to the same firmware pair. One image serving
six boards means a table, and it's at file offset `0x1fec` with a `0x4a4` stride.
Six board profiles, selected at runtime.

The decode hinges on one bit. Each IR output descriptor has a pin-mask word, and
bit 8 means "this channel is populated on this board." Without that reading the
six blocks look like arbitrary reorderings of the same data. With it they become
board profiles, and the counts match physical hardware exactly:

| block | IR outputs | user UARTs | board |
|---|---|---|---|
| 2 | 5 | 2 | **EA1** — 4 rear jacks + 1 internal blaster |
| 3 | 7 | 3 | **EA3** — 6 rear jacks + 1 internal blaster |
| 0, 1, 4, 5 | 9 | 2 | EA5 / TR1 / amp1 |

Two independent facts confirm the EA3 assignment: the owner counts six IR jacks,
and a live EA3's `ioserver` opens three user-serial sockets where an EA1 opens
two.

Relays and contacts fall out of the same table, from two four-pin groups where
bit 8 means the same thing. The EA1 populates none of the eight, and an EA1 has no
relays and no contacts. The EA3 populates exactly one from each group, and an EA3
has one relay and one contact. The nine-output blocks populate all eight, which is
the HC800/HC250 complement of 4 + 4.

Three separate predictions matching known hardware is what makes the decode
trustworthy rather than a story. What it still doesn't settle is which of the two
groups is relays and which is contacts. Both are four wide and nothing
distinguishes an input from an output.

### The selector, and a correction

The vendor picks its block from an analogue board-ID strap into the MCU's ADC.
Disassembling that function corrected two things I'd believed.

The five conversions are a settle-and-discard loop, not an average. The scratch
word gets overwritten by every read and nothing sums it, so only the fifth reading
reaches the arithmetic. The averaging is in hardware, via
`ADCHardwareOversampleConfigure(ADC0, 8)`.

The arithmetic is then `id = adc / 250` with rounding, and the id is used directly
as the block index, multiplied by the `0x4a4` stride at 21 separate sites in the
image.

Which fully determines the strap mapping, on a 12-bit ADC against 3.3 V:

| block | ADC counts | volts on PB4 | board |
|---|---|---|---|
| 2 | ~500 | ~0.40 V | EA1 |
| 3 | ~750 | ~0.60 V | EA3 |

Those voltages are predictions, not measurements. Nobody has put a meter on PB4.
Confirming them would let our firmware drop its compile-time board switch and ship
one image for both boards, the way the vendor does.

## IR, confirmed in both directions

`IROUT_SEND` does almost no conversion. A six-byte header, then raw Pronto/CCF
words passed straight through, big-endian. Burst durations are carrier periods,
not microseconds, which is a unit the format's own documentation makes easy to get
wrong.

This frame emitted 38 kHz IR on output 1 of a live EA1:

```
TX 10 02 66 11 00 00 16 01 00 00 01 00 01 00 00 00 6d 00 02 00 00 01 57 00 ac 00 16 00 16 d1
RX 10 02 68 11 02 ... 00     IROUT_STATUS, status 0
```

Capture independently confirmed the same unit derivation. Against a real NEC
remote, 40 frames, all checksums good:

```
0515 | 0001 8156 00aa 8016 0016 8014 003f 8016 0016 ...
^^^^   carrier period = 1301 ticks
       M342  S170  M22  S22  M20  S63 ...
```

`342 periods / 9 ms` is 38000 Hz exactly, and the cells decode as textbook NEC: a
9.0 ms + 4.47 ms leader, then 0.58 ms marks with 0.58/1.66 ms spaces. NEC
specifies 9.0/4.5, 560 µs and 1690 µs. Two independent datapaths agreeing on the
unit is what makes it a fact rather than a reading.

The capture path had its own trap, and it's a good one. The opcode named
`IRIN_SET_CAPTURE_TO_INIT` isn't an arm. Despite the name it *resets* capture
state and zeroes the input pin mask, so on its own it enables nothing. An early
version of the probe tool re-armed every three seconds, which disabled capture
continuously and produced five consecutive false negatives before anyone suspected
the tool rather than the hardware.

## Contacts are pulled, never pushed

Measured on a live EA3 by shorting the input and watching both edges: the bitmask
is one bit per contact, and a closed contact reads 1.

The part that matters for anyone writing a replacement daemon is that the MCU
doesn't volunteer contact changes. In a 45-second window with the contact held
closed, not one unsolicited frame appeared. The vendor's `ioserver` re-reads only
when its director asks for a MIB, and those polls were one to two minutes apart.

So a host has to run its own poll loop and derive edges by comparison. Waiting for
an event will never fire.

## The silicon facts that would have produced silent failures

Recorded because each of these makes firmware that fails with no diagnostic at
all.

- The part is 64 KB flash / 24 KB SRAM, not the 256 KB/32 KB of larger TM4C123
  parts. A linker script with the bigger numbers puts the stack pointer past the
  top of real SRAM, and the firmware hard-faults on its first push, before any code
  can report anything.
- This part has no PWM module. The IR carrier has to come from a general-purpose
  timer's capture/compare pin.
- GPIO AHB apertures are unmapped at reset until the matching bit is set in
  `SYSCTL_GPIOHBCTL`. A UART muxed through an AHB aperture without that simply
  never comes up: reads return zero, writes vanish, nothing errors.
- The bootloader doesn't set VTOR, so an application at `0x1000` has to relocate
  its own vector table.
- A complete vector table is 123 entries. A short one doesn't fault. Peripheral
  interrupts just vector into whatever code follows it in flash.

The package question got settled by the data rather than by guessing. A ten-entry
descriptor table in the image assigns every channel to a pin, and all ten land
exactly on the documented CCP0 pin for their own timer. A 10/10 correlation isn't
coincidence, and channel 2's use of `PB6`, which exists on the PM package and not
on the PZ, settles which package is fitted.
