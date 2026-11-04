---
title: IO-MCU pin maps
description: Every IR output, relay, contact, combo-jack serial line and board strap on the EA1, EA3, EA5 and HC-800 IO microcontrollers, decoded from Control4's stock images and checked against TI's pin tables.
sidebar:
  order: 3
---

The pin each IO function sits on, per board, read out of Control4's stock IO-MCU
application images. These are the pins in each EA model's `.dtsi`
(`board/ea/<model>/<model>.dtsi`, the shared receiver and heartbeat in `ea.dtsi`)
and in the HC-800's `board/hc800/acpi/ssdt-iomcu.asl`. Inferred rows are marked
there too. The protocol, bring-up and
history are on [The IO microcontroller](/shared/io-mcu/).

Facts only: table layouts, addresses, pin numbers, and the evidence for each. No
vendor code is reproduced.

## Confidence levels

| Level | Meaning |
|---|---|
| **HW** | Decoded, and the behaviour has also been seen on a live unit. |
| **decoded** | Read from the board's table *and* traced through the code that uses it. |
| **inferred** | A reasoned reading the image doesn't state outright. Check before relying on it. |

Every pin's function was checked against TI's pin tables: the
[TM4C1231D5PM data sheet](https://www.ti.com/lit/ds/symlink/tm4c1231d5pm.pdf), Table 10-2
(64LQFP, rev. 15842.2741, June 2014), and StellarisWare's `pin_map.h`, section
`PART_LM3S1162` (TI's LM3S1162 data sheet link is dead). No pin in any table below
contradicts its claimed function.

:::caution[Relays switch real loads]
A relay pin is only as certain as its row says. On every board below, the relay
pins are *decoded*: configured as outputs, driven low at boot, and driven high by the
relay-on command. That high = coil energised is **inferred**; it depends on the
driver stage. On the HC-800, toggling relay indices 0–3 moves four relays (**HW**),
but which pin is which panel relay has not been measured.
:::

## Images

| Board | Image | sha256 |
|---|---|---|
| EA1, EA3, EA5 | `660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin` | `e6045980…52a5` |
| amp1 | `660-00063_TM4C1231D5_IO_Processor_Release_1.0.41.093e513.bin` | `dc8a2f57…c9c` |
| HC-800, HC-250 | `700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin` | `83bd6b72…cec` |

All from `/control4/firmware/io/` in a stock EA rootfs (`recfs.tar.xz`). The TM4C
files are raw flash from `0x0000`, so file offset = flash address. The LM3S file
has a 256-byte header, so flash = file − `0x100`. Addresses below are flash
addresses. The amp1 image carries a byte-identical profile table at the same
address.

## How each image picks its board

Both parts read an analogue strap at boot and use the result directly as a
profile-block index. There is no range check in either image.

| | TM4C1231D5 (EA) | LM3S1162 (HC) |
|---|---|---|
| Routine | `0x1878` | `0x519c` |
| Input | ADC0, step channel 10 = **AIN10 = PB4** | ADC0, step channel 0 = **ADC0** |
| Oversampling | ×8 in hardware | ×8 in hardware |
| Conversions | 5, only the last one is used | 5, only the last one is used |
| Index | `round(adc / 250)` | `round(adc / 85)` |
| Block table | `0x1fec`, 6 × `0x4a4` | `0x10b8`, 4 × `0x3d4` |
| Stored at | RAM struct `+0x2c` | `0x20001e90` |

`round` here is integer divide, plus one when the remainder is at least half the
divisor (126 of 250, 42 of 85).

Predicted strap voltages, **not measured on any board**:

| Board | Block | Counts | Volts |
|---|---|---|---|
| EA1 | 2 | ~500 of 4095 | ~0.40 V (3.3 V ref) |
| EA3 | 3 | ~750 | ~0.60 V |
| EA5 | 4 (inferred) | ~1000 | ~0.81 V |
| HC-800 | 2 | ~170 of 1023 | ~0.50 V (if the 3.0 V internal ref) |
| HC-250 | 1 | ~85 | ~0.25 V |

`IoProcMultiConfig` therefore means multi-*board*, not multi-processor. The
`Processor: LM3S615 / LM3S815 / LM3S811 / LM3S1162` strings only label a version
banner (DID1 PARTNO `0x26` / `0x34` / `0x32` / `0xc4`, routine at `0x71d6`), and
the block is chosen by the strap alone.

The stock images can't report their index. The TM4C formats a `deviceConfigId: %d`
banner at boot, but its output call is compiled out (`0x8312`, `0x833c` are NOPs).
The only ways to learn a board's block are the strap voltage or the jack behaviour.

## TM4C1231D5 profile block

`0x4a4` bytes per block. A pin word is `bits 0..7 = pin mask, bit 8 = populated`.

| Offset | Size | Contents | Used by |
|---|---|---|---|
| `+0x000` | `0x18` | IR receiver `{gpio, pin, timer, exc A, 0xff, half}` | `0x1470` |
| `+0x018` | `0x1c` | Front blaster `{gpio, pin, timer, exc A, exc B, 0xff, half}` | `0x166c` |
| `+0x034` | 16 × `0x1c` | Rear IR outputs, same record. The list ends at the first unpopulated record. | `0x13f8`, `0x166c` |
| `+0x1f4` | 32 × 8 | Contacts `{gpio, pin}`: GPIO **inputs** | `0x1708`, `0x1970` |
| `+0x2f4` | 16 × 8 | Relays `{gpio, pin}`: GPIO **outputs**, low at boot | `0x1764`, `0x1cec` |
| `+0x374` | 8 | Spare output (`PD0`, unpopulated in every block) | `0x1cae` |
| `+0x37c` | 8 | Heartbeat output (`PC7`) | `0x17c4`, `0x1b62` |
| `+0x384` | 3 × 8 | Combo-jack line A, per user UART | `0x14c0`, `0x1baa` |
| `+0x39c` | 3 × 8 | Combo-jack line B, per user UART | `0x1518`, `0x1baa` |
| `+0x3b4` | 6 × `0x28` | Peripheral records (below) | `0x1804` |

Exception numbers are IRQ + 16. `half` (byte, 0 everywhere) picks timer A, so
every IR pin is a `CCP0`.

**The front blaster gets the last channel index, not the first.** `0x166c` numbers
the rear records 0, 1, 2… and then gives the `+0x018` record the next index. The
same index is kept at `+0x2e` and checked by the per-channel interrupt setup at
`0x1570`, which has handlers for 8 rear channels plus the blaster. So the
`IROUT_SEND` bit for the blaster is the rear count: bit 4 on EA1, 6 on EA3, 8 on
EA5. That matches what the kernel driver already assumes (`blaster_bit = highest
rear channel + 1`). An earlier version of [the profile table
section](/shared/io-mcu/#the-per-board-profile-table) listed `PD4` as channel 0,
which was wrong.

**IR pin-mux values are not in the table.** `0x3e74` switches on the timer base and
hard-codes each `GPIOPinConfigure` word, in TivaWare encoding
`port << 16 | (pin × 4) << 8 | function`. All ten are function 7:

| Timer | Word | Pin | Data sheet |
|---|---|---|---|
| TIMER0 A | `0x00011807` | PB6 | PB6 PMC 7 = T0CCP0 ✓ |
| TIMER1 A | `0x00050807` | PF2 | PF2 PMC 7 = T1CCP0 ✓ |
| TIMER2 A | `0x00010007` | PB0 | PB0 PMC 7 = T2CCP0 ✓ |
| TIMER3 A | `0x00010807` | PB2 | PB2 PMC 7 = T3CCP0 ✓ |
| WTIMER0 A | `0x00021007` | PC4 | PC4 PMC 7 = WT0CCP0 ✓ |
| WTIMER1 A | `0x00021807` | PC6 | PC6 PMC 7 = WT1CCP0 ✓ |
| WTIMER2 A | `0x00030007` | PD0 | PD0 PMC 7 = WT2CCP0 ✓ |
| WTIMER3 A | `0x00030807` | PD2 | PD2 PMC 7 = WT3CCP0 ✓ |
| WTIMER4 A | `0x00031007` | PD4 | PD4 PMC 7 = WT4CCP0 ✓ |
| WTIMER5 A | `0x00031807` | PD6 | PD6 PMC 7 = WT5CCP0 ✓ |

The same switch also carries `PF3` (T1CCP1) and `PB1` (T2CCP1) for the B halves.
No board's table selects them, and both pins are used as GPIOs.

**Peripheral record**, `0x28` bytes:
`{flags, base, rx pinconfig, tx pinconfig, exc, baud, rx gpio, rx pin, tx gpio, tx pin}`.
`0x1804` brings records up in order and **stops at the first one whose flags byte 0
is 0**. Flags `0x00010001` = host link, `0x02000001` = user port. Every block
then carries two disabled templates, never reached: `I2C1` on PA6/PA7 (PMC 3, flags
`0x100`) and `SSI0` on PA4/PA5 (PMC 2, flags `0x200`).

**Contacts read closed = pin low.** `0x1970` sets a contact's bit when the pin does
*not* read high.

**`PF0` is a locked pin** (NMI). The firmware unlocks it at `0x829a`: it writes the
lock key to `GPIOLOCK` (`0x40025520`), sets bit 0 of `GPIOCR`, and clears its
`AFSEL`. A replacement firmware must do the same before PF0 works as a contact.

### Combo-jack lines

Each user UART owns a pair of outputs, A (`+0x384[n]`) and B (`+0x39c[n]`), driven
by `0x1baa(port, mode)`:

| Mode | A | B | Set by |
|---|---|---|---|
| 0 | low | low | at boot; `0xA0` with enable 0; opcodes `0x11` / `0x14` |
| 1 | **high** | low | `0xA0` with enable 1; `0xA3` with argument 0 |
| 2 | low | **high** | `0xA3` with argument 1 |

The modes are **decoded**. Mode 0 = jack in IR, mode 1 = serial is **inferred**. Mode
2 = serial with RX/TX swapped (the null-modem option stock offers per port) is also
**inferred**; `0xA3` is its only source.

## EA1 — block 2

| Function | Index | Port | Pin | Peripheral / alt function | Confidence | Evidence |
|---|---|---|---|---|---|---|
| IR out (jack 1) | bit 0 | B | 6 | TIMER0A, T0CCP0, PMC 7, IRQ 19 | decoded | `0x2934+0x034`; `0x3e74` |
| IR out (jack 2) | bit 1 | F | 2 | TIMER1A, T1CCP0, PMC 7, IRQ 21 | decoded | `+0x050` |
| IR out (jack 3) | bit 2 | B | 0 | TIMER2A, T2CCP0, PMC 7, IRQ 23 | decoded | `+0x06c` |
| IR out (jack 4) | bit 3 | B | 2 | TIMER3A, T3CCP0, PMC 7, IRQ 35 | decoded | `+0x088` |
| IR blaster (front) | bit 4 | D | 4 | WTIMER4A, WT4CCP0, PMC 7, IRQ 102 | decoded | `+0x018`; driver finds 5 emitters (**HW**) |
| IR receiver | 0 | D | 6 | WTIMER5A, WT5CCP0, PMC 7, IRQ 104 | HW | `+0x000`; captures work |
| Host UART RX / TX | — | A | 0 / 1 | UART0, PMC 1, IRQ 5 | HW | `+0x3b4` |
| User serial 0 RX / TX | 0 | E | 4 / 5 | UART5, PMC 1, IRQ 61 | decoded | `+0x3dc`, words `0x41001` / `0x41401` |
| User serial 1 RX / TX | 1 | E | 0 / 1 | UART7, PMC 1, IRQ 63 | decoded | `+0x404`, words `0x40001` / `0x40401` |
| Combo line A, serial 0 | 0 | E | 2 | GPIO out | decoded | `+0x384` |
| Combo line B, serial 0 | 0 | B | 1 | GPIO out | decoded | `+0x39c` |
| Combo line A, serial 1 | 1 | E | 3 | GPIO out | decoded | `+0x38c` |
| Combo line B, serial 1 | 1 | B | 3 | GPIO out | decoded | `+0x3a4` |
| Heartbeat | — | C | 7 | GPIO out, toggled every 100 main-loop ticks | decoded | `+0x37c`; `0x83e2` |
| Board strap | — | B | 4 | AIN10 (analog) | decoded | `0x1878` |
| Relays, contacts | — | — | — | none populated | decoded | `+0x1f4`, `+0x2f4` all clear bit 8 |

Jack *N* = bit *N−1* is the device tree's convention. It's proven on the HC-800
with a learner (bit 0 out of jack 1, bit 5 out of jack 6), not yet on any EA.

## EA3 — block 3

As EA1 for bits 0–3, the receiver, the host link, user serial 0–1, their combo
lines, the heartbeat and the strap. Additional or different:

| Function | Index | Port | Pin | Peripheral / alt function | Confidence | Evidence |
|---|---|---|---|---|---|---|
| IR out (jack 5) | bit 4 | D | 0 | WTIMER2A, WT2CCP0, PMC 7, IRQ 98 | decoded | `0x2dd8+0x0a4` |
| IR out (jack 6) | bit 5 | D | 2 | WTIMER3A, WT3CCP0, PMC 7, IRQ 100 | decoded | `+0x0c0` |
| IR blaster (front) | bit 6 | D | 4 | WTIMER4A, WT4CCP0, PMC 7, IRQ 102 | decoded | `+0x018` |
| User serial 2 RX / TX | 2 | C | 4 / 5 | UART4, PMC 1, IRQ 60 | decoded | `+0x42c`, words `0x21001` / `0x21401`; stock opens 3 user sockets (**HW**) |
| Combo line A, serial 2 | 2 | A | 7 | GPIO out | decoded | `+0x394` |
| Combo line B, serial 2 | 2 | B | 5 | GPIO out | decoded | `+0x3ac` |
| Relay 1 | 0 | A | 2 | GPIO out, low at boot, on = high | decoded | `+0x2f4` |
| Contact 1 | 0 | F | 0 | GPIO in (locked pin, unlocked at `0x829a`), closed = low | decoded, index 0 is HW | `+0x1f4`; contact edges seen on bit 0 |

PC4 is UART4 RX here, which is why the `PC4`/WTIMER0 IR record is unpopulated on
this board.

## EA5 — block 4 (inferred)

EA5 is 8 rear IR jacks + blaster, 4 relays, 4 contacts and 2 combo jacks. Four
blocks fit that: 0, 1, 4 and 5. Blocks 0, 1 and 5 are byte-identical, and block 4
differs from them **only in the order of rear channels 4–7**. Block 4 extends the
EA1/EA3 order (`PD0`, `PD2` first), which is why it's the likely one. The image
doesn't name a board, so the choice is **inferred**.

| Function | Index | Port | Pin | Peripheral / alt function | Confidence | Evidence |
|---|---|---|---|---|---|---|
| IR out (jacks 1–4) | bits 0–3 | B, F, B, B | 6, 2, 0, 2 | as EA1 | decoded | `0x327c+0x034..0x088`, same in all four candidate blocks |
| IR out (jack 5) | bit 4 | D | 0 | WTIMER2A, WT2CCP0, PMC 7, IRQ 98 | inferred (block) | block 4 `+0x0a4`; blocks 0/1/5: PC4 |
| IR out (jack 6) | bit 5 | D | 2 | WTIMER3A, WT3CCP0, PMC 7, IRQ 100 | inferred (block) | block 4 `+0x0c0`; blocks 0/1/5: PC6 |
| IR out (jack 7) | bit 6 | C | 4 | WTIMER0A, WT0CCP0, PMC 7, IRQ 94 | inferred (block) | block 4 `+0x0dc`; blocks 0/1/5: PD0 |
| IR out (jack 8) | bit 7 | C | 6 | WTIMER1A, WT1CCP0, PMC 7, IRQ 96 | inferred (block) | block 4 `+0x0f8`; blocks 0/1/5: PD2 |
| IR blaster (front) | bit 8 | D | 4 | WTIMER4A, WT4CCP0, PMC 7, IRQ 102 | decoded | `+0x018`, all candidates |
| IR receiver | 0 | D | 6 | WTIMER5A, WT5CCP0, PMC 7, IRQ 104 | decoded | `+0x000` |
| Host UART RX / TX | — | A | 0 / 1 | UART0, PMC 1, IRQ 5 | decoded | `+0x3b4` |
| User serial 0 RX / TX | 0 | E | 4 / 5 | UART5, PMC 1, IRQ 61 | decoded | `+0x3dc` |
| User serial 1 RX / TX | 1 | E | 0 / 1 | UART7, PMC 1, IRQ 63 | decoded | `+0x404` |
| Combo lines, serial 0 | 0 | E / B | 2 / 1 | GPIO out (A / B) | decoded | `+0x384` / `+0x39c` |
| Combo lines, serial 1 | 1 | E / B | 3 / 3 | GPIO out (A / B) | decoded | `+0x38c` / `+0x3a4` |
| Relay 1 | 0 | A | 2 | GPIO out, on = high | decoded | `+0x2f4` |
| Relay 2 | 1 | A | 3 | GPIO out, on = high | decoded | `+0x2fc` |
| Relay 3 | 2 | A | 4 | GPIO out, on = high | decoded | `+0x304` |
| Relay 4 | 3 | A | 5 | GPIO out, on = high | decoded | `+0x30c` |
| Contact 1 | 0 | F | 0 | GPIO in (locked pin), closed = low | decoded | `+0x1f4` |
| Contact 2 | 1 | F | 1 | GPIO in, closed = low | decoded | `+0x1fc` |
| Contact 3 | 2 | F | 3 | GPIO in, closed = low | decoded | `+0x204` |
| Contact 4 | 3 | F | 4 | GPIO in, closed = low | decoded | `+0x20c` |
| Heartbeat | — | C | 7 | GPIO out | decoded | `+0x37c` |
| Board strap | — | B | 4 | AIN10 | decoded | `0x1878` |

Relays and contacts are identical in all four candidate blocks, so they don't
depend on which block EA5 uses. Only jacks 5–8 do. The PA2–PA5 relays are SSI0's
pins and PA4/PA5 also appear in the disabled SSI0 template. The template is never
brought up, so there's no conflict.

## LM3S1162 config block

`0x3d4` bytes per block. Pin words are `bits 0..7 = mask`, and byte 1 is `1` when
populated, or `0x80 | n` for PWM output *n*.

| Offset | Size | Contents | Used by |
|---|---|---|---|
| `+0x000` | 3 × 12 | Timer records `{base, exc, 0xff}`: TIMER2 (IRQ 23), TIMER0 (IRQ 19), TIMER3 (IRQ 35). No pins. | `0x4f70` |
| `+0x024` | 20 | IR receiver `{gpio, pin, timer, exc, …}` | `0x4ef0` |
| `+0x038` | 16 × 24 | IR outputs: three `{gpio, pin}` per output (A, B, C). The list ends at the first group whose A gpio is 0. | `0x4e54`, `0x555e`, `0x5520` |
| `+0x1b8` | 2 × 8 | Carrier PWM pins | `0x4fb8`, `0x534c` |
| `+0x1c8` | 32 × 8 | Contacts: GPIO **inputs** | `0x4ffe`, `0x52b8` |
| `+0x2c8` | 16 × 8 | Relays: GPIO **outputs**, low at boot | `0x5050`, `0x565c` |
| `+0x348` | 8 | Spare output | `0x50b6` |
| `+0x350` | 8 | Heartbeat output | `0x50ee`, `0x54fc` |
| `+0x358` | 2 × 8 | Serial enable, one per user UART, raised by `0xA0` | `0x5606` |
| `+0x368` | 3 × `0x24` | UART records `{flags, base, rx word, tx word, baud, rx gpio, rx pin, tx gpio, tx pin}` | `0x513a`, `0x5886` |

`0x534c` maps the PWM byte `0x80..0x87` to StellarisWare's `PWM_OUT_0..7`.

**The carrier comes from just two PWM pins, and each jack is gated onto one of
them by GPIO.** `0x4e54` sets lines A and B high and C low at boot. `0x555e(jack,
enable, select)` then drives:

| select | enable | A | B |
|---|---|---|---|
| 0 | 1 | **low** | high |
| 1 | 1 | high | **low** |
| any | 0 | high | high |

So A and B read as active-low enables for the two carrier sources (**inferred**).
Pairing select 0 with the first PWM record (PD1) is also **inferred**. C is only
ever written low, at boot and by the end-of-send loop in `0x471a`. No path in the
IR code raises it, and its purpose is unknown.

The vector table's PWM generator 0/1/2 handlers match: PWM1 is generator 0 and
PWM5 is generator 2. Generator 1 has a handler and no pin, so presumably it does
timing. The earlier "three generators × two outputs = six jacks" reading on
[The IO microcontroller](/shared/io-mcu/) was wrong.

## HC-800 — block 2

Block 2 is the HC-800's because it is the only block with 4 inputs and 4 outputs
populated and 7 IR groups. That matches 6 rear jacks, the front blaster at bit 6,
4 relays and 4 contacts, and closing contact 4 on a live unit sets bit 3 (**HW**).

| Function | Index | Port | Pin | Peripheral / alt function | Confidence | Evidence |
|---|---|---|---|---|---|---|
| IR carrier 0 | — | D | 1 | PWM1 (generator 0 B), AFSEL | decoded | `0x1860+0x1b8`, byte `0x81`; pin_map PWM1 = PD1 ✓ |
| IR carrier 1 | — | F | 3 | PWM5 (generator 2 B), AFSEL | decoded | `+0x1c0`, byte `0x85`; pin_map PWM5 = PF3 ✓ |
| IR jack 1 gate A / B / C | bit 0 | C / C / F | 7 / 6 / 4 | GPIO out | decoded; bit 0 = jack 1 is HW | `+0x038..0x048` |
| IR jack 2 gate A / B / C | bit 1 | C / C / F | 5 / 4 / 5 | GPIO out | decoded | `+0x050..0x060` |
| IR jack 3 gate A / B / C | bit 2 | B / B / A | 4 / 5 / 7 | GPIO out | decoded | `+0x068..0x078` |
| IR jack 4 gate A / B / C | bit 3 | B / F / A | 6 / 2 / 6 | GPIO out | decoded | `+0x080..0x090` |
| IR jack 5 gate A / B / C | bit 4 | E / E / D | 0 / 1 / 6 | GPIO out | decoded | `+0x098..0x0a8` |
| IR jack 6 gate A / B / C | bit 5 | E / E / D | 2 / 3 / 7 | GPIO out | decoded; bit 5 = jack 6 is HW | `+0x0b0..0x0c0` |
| IR blaster gate A / B | bit 6 | F / D | 0 / 5 | GPIO out (no C) | decoded; bit 6 = front is HW | `+0x0c8..0x0d0` |
| IR receiver | 0 | D | 4 | CCP3 = TIMER1 B, AFSEL; record IRQ 22 | decoded | `+0x024`; pin_map CCP3 = PD4 ✓ |
| Host UART RX / TX | — | A | 0 / 1 | UART0, AFSEL | HW | `+0x368`, flags `0x00010001`; 115200 |
| Relay 1 | 0 | A | 5 | GPIO out, low at boot, on = high | decoded; index 0 moves a relay (HW) | `+0x2c8` |
| Relay 2 | 1 | A | 4 | GPIO out, on = high | decoded; HW as above | `+0x2d0` |
| Relay 3 | 2 | A | 3 | GPIO out, on = high | decoded; HW as above | `+0x2d8` |
| Relay 4 | 3 | A | 2 | GPIO out, on = high | decoded; HW as above | `+0x2e0` |
| Contact 1 | 0 | B | 0 | GPIO in, closed = low | decoded | `+0x1c8`; `0x52b8` |
| Contact 2 | 1 | B | 1 | GPIO in, closed = low | decoded | `+0x1d0` |
| Contact 3 | 2 | B | 2 | GPIO in, closed = low | decoded | `+0x1d8` |
| Contact 4 | 3 | B | 3 | GPIO in, closed = low | HW (bit 3) | `+0x1e0` |
| Heartbeat | — | F | 1 | GPIO out, toggled from the main loop | decoded | `+0x350`; `0x7238..0x7268` |
| Board strap | — | — | — | ADC channel 0 (dedicated analog pin) | decoded | `0x519c` |
| User serial | — | — | — | none: the rear DB9s are host UARTs | HW | serial-enable slots (PA5/PA4) unpopulated |

UART1 (PD2/PD3) is brought up with flags `0x01000001`. Its role byte is 1, neither
host (0) nor user (2), so `0x5886` gives it no buffers. Its purpose is not
decoded. UART2 is disabled (`0x01000000`).

The gate pins are plain GPIOs. On this part PB4–PB6, PC4–PC7, PA6, PA7 and PF5 also
carry comparator or CCP functions, and PE0–PE3 carry SSI1. Relays PA2–PA5 are
SSI0's pins, contacts PB0/PB1 are CCP0/CCP2, and PB2/PB3 are I2C0. None is
selected here. The LM3S1162 has no `GPIOPCTL`: `AFSEL` alone picks the single
alternate function, so there is no function number to choose.

## HC-250 and the other LM3S blocks

Not openHC targets, recorded because they explain the image.

| Block | Strap | Board | IR | Relays / contacts | User serial |
|---|---|---|---|---|---|
| 0 | ~0 V | unidentified | 6 rear + front (same pins as HC-800) | slots present, none populated | UART1 PD2/PD3 + UART2 PG0/PG1, enables PB0 / PB1 |
| 1 | ~0.25 V | **HC-250** (inferred) | 4 rear (HC-800 jacks 1–4 pins) + front (PF0/PD5) | relay PE0, contact PE1 | UART1 + UART2, enables PB0 / PB1 |
| 2 | ~0.50 V | **HC-800** | above | above | none |
| 3 | ~0.75 V | fallback | none | none | UART1 + UART2 templates, enables unpopulated |

Block 1 matches the HC-250's published I/O exactly: four IR jacks with ports 1–2
switchable to serial, one contact, one relay.

## Machine-readable

Port letter, pin number, alternate function (PMC on TM4C, `-` where the pin is
GPIO or AFSEL-only). `ir` lists rear jacks in bit order, then `blaster`.

```yaml
ea1:   # TM4C block 2
  ir: [{port: B, pin: 6, timer: TIMER0A, pmc: 7}, {port: F, pin: 2, timer: TIMER1A, pmc: 7},
       {port: B, pin: 0, timer: TIMER2A, pmc: 7}, {port: B, pin: 2, timer: TIMER3A, pmc: 7}]
  blaster: {port: D, pin: 4, timer: WTIMER4A, pmc: 7}
  ir_rx: {port: D, pin: 6, timer: WTIMER5A, pmc: 7}
  uart: [{uart: 5, rx: E4, tx: E5, pmc: 1, mux_a: E2, mux_b: B1},
         {uart: 7, rx: E0, tx: E1, pmc: 1, mux_a: E3, mux_b: B3}]
  relay: []
  contact: []
  heartbeat: C7
  strap: {pin: B4, adc: AIN10, block: 2}
ea3:   # TM4C block 3
  ir: [B6, F2, B0, B2, {port: D, pin: 0, timer: WTIMER2A, pmc: 7}, {port: D, pin: 2, timer: WTIMER3A, pmc: 7}]
  blaster: D4
  ir_rx: D6
  uart: [{uart: 5, rx: E4, tx: E5, mux_a: E2, mux_b: B1},
         {uart: 7, rx: E0, tx: E1, mux_a: E3, mux_b: B3},
         {uart: 4, rx: C4, tx: C5, mux_a: A7, mux_b: B5}]
  relay: [A2]
  contact: [F0]        # locked pin: unlock before use
  heartbeat: C7
  strap: {pin: B4, adc: AIN10, block: 3}
ea5:   # TM4C block 4 (inferred); blocks 0/1/5 swap ir[4..7] to C4, C6, D0, D2
  ir: [B6, F2, B0, B2, D0, D2, {port: C, pin: 4, timer: WTIMER0A, pmc: 7}, {port: C, pin: 6, timer: WTIMER1A, pmc: 7}]
  blaster: D4
  ir_rx: D6
  uart: [{uart: 5, rx: E4, tx: E5, mux_a: E2, mux_b: B1},
         {uart: 7, rx: E0, tx: E1, mux_a: E3, mux_b: B3}]
  relay: [A2, A3, A4, A5]
  contact: [F0, F1, F3, F4]
  heartbeat: C7
  strap: {pin: B4, adc: AIN10, block: 4}
hc800: # LM3S1162 block 2
  carrier: [{port: D, pin: 1, pwm: 1}, {port: F, pin: 3, pwm: 5}]
  ir:  # gates a, b (active low, inferred), c (unknown, held low)
    - {a: C7, b: C6, c: F4}
    - {a: C5, b: C4, c: F5}
    - {a: B4, b: B5, c: A7}
    - {a: B6, b: F2, c: A6}
    - {a: E0, b: E1, c: D6}
    - {a: E2, b: E3, c: D7}
  blaster: {a: F0, b: D5}
  ir_rx: {port: D, pin: 4, ccp: 3, timer: TIMER1B}
  uart: []
  relay: [A5, A4, A3, A2]
  contact: [B0, B1, B2, B3]
  heartbeat: F1
  strap: {adc: 0, block: 2}
```

## Open gaps

1. **Which block EA5 uses.** 4 is inferred, and 0, 1 and 5 are the alternatives.
   It only affects rear jacks 5–8. Settle it by measuring PB4 (block 4 ≈ 0.81 V,
   block 5 ≈ 1.0 V, block 1 ≈ 0.2 V, block 0 ≈ 0 V), or by sending bits 4–7 one at
   a time and watching which jack lights.
2. **No strap voltage has been measured** on any board. All are predictions from
   the arithmetic.
3. **Jack *N* = bit *N−1* on the EA family** is the DT convention, proven only on
   the HC-800.
4. **Combo-jack modes.** Mode 1 = serial and mode 2 = swapped (null-modem) are
   inferred. A loopback with `0xA3` 0/1 would settle it.
5. **HC-800 gate polarity, and which PWM pairs with A vs B**, are inferred. The
   third line, C, is undecoded.
6. **HC-800 UART1** (role 1, no buffers): purpose unknown.
7. **Relay coil polarity** (pin high = energised) is inferred on every board. The
   HC-800 relay pins are decoded, but which pin drives which panel relay has not
   been measured.
8. **LM3S block 0**'s board is unidentified. It isn't an openHC target.

## Method

1. Extracted `control4/firmware/io/` from a stock EA `recfs.tar.xz`, read-only.
   No controller or MCU was touched.
2. Disassembled both application images as raw Thumb from `0x1000`.
3. Found each block table by the code that multiplies a board index by its stride
   (`0x4a4` on the TM4C, `0x3d4` on the LM3S), then followed every routine that
   reads a field. That gives direction (input / output / alternate), initial level,
   and on-level for each group.
4. Identified the StellarisWare / TivaWare driverlib helpers by their register
   offsets (`DIR`, `AFSEL`, the pad-config block, the masked `DATA` write,
   `GPIOPCTL`), which identifies "pin type input", "pin type output" and so on.
5. Matched groups to commands through the opcode dispatchers (TM4C `0x6754`, LM3S
   `0x3e5e`): relay set/pulse, contact poll, `UART_SET_CONTROL` (`0xA0`),
   `0xA3`, `IROUT_SEND`.
6. Checked every pin against TI's tables (TM4C data sheet Table 10-2, StellarisWare
   `pin_map.h` `PART_LM3S1162`).
7. Marked as **HW** only what an earlier live test on this project already showed.
