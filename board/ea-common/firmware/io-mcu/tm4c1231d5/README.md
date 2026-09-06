# io-mcu — clean-room IO-processor firmware

Replacement firmware for the Control4 IO microcontroller (TI Tiva TM4C1231D5 on
EA1/EA3/EA5, Stellaris LM3S1162 on HC800/HC250), wire-compatible with the
protocol `ohc-iod` already speaks so the host side needs no changes.

## Boards

```sh
make fw               # EA1 (default)
make fw BOARD=ea3     # EA3
make all-boards       # both, into build/ea1/ and build/ea3/
```

`BOARD` selects a compile-time profile in
[`include/board_profile.h`](include/board_profile.h):

| | IR outputs | IR jacks | user serial | relays | contacts | vendor block |
|---|---|---|---|---|---|---|
| **ea1** | 5 | 4 | 2 | 0 | 0 | 2 |
| **ea3** | 7 | 6 | 3 | 1 | 1 | 3 |

The extra IR output on each board is an internal front blaster, which is why the
output count is one above the rear-jack count.

Only counts vary. Both boards use the same pins in the same order for the
channels they populate, so a profile is a **prefix length**, not a remapping,
and `IROUT_SEND`'s `output_mask` stays dense: `0x1f` on EA1, `0x7f` on EA3.

**The images are not interchangeable.** The stock firmware ships one binary for
ea1/ea3/ea5 and picks a profile at runtime from an ADC strap (ADC0/AIN10 on PB4,
`round(adc/250)`); we know which block each board uses but not the ADC value
that selects it, so we emit one image per board instead. Measure that strap on a
live EA1 and EA3 and this collapses back to a single image — `board_profile.h`
is already shaped for it.

Everything in the table above was decoded from the stock image rather than
assumed; the evidence is in
[docs/io-mcu-firmware.md](../../docs/io-mcu-firmware.md#the-per-board-profile-table).

**Clean room.** Everything here is written from *observed behaviour* documented
in [docs/io-mcu-firmware.md](../../docs/io-mcu-firmware.md) — frames captured off
a live unit, and the semantics that had to be true for those frames to make
sense. No vendor code is copied, disassembled into, or translated.

## Why there is no SDK here

Evaluated properly (Zephyr, libopencm3, TivaWare, bare-metal), then each
conclusion attacked by independent verifiers. **Decision: stay bare-metal, and
vendor TivaWare's BSD-3 headers as a read-only correctness oracle we never
link.**

The deciding fact: **this part has no PWM module.** TM4C1231D5PM's DC1/DC3/DC5
PWM-presence bits read 0, so the IR carrier has to come from a general-purpose
timer's CCP output. Not one of the candidates ships a driver that does that, nor
one for edge-time capture. Every option leaves the carrier, the burst state
machine and the capture ISR to us — we would write identical register code, with
a licence and a build system stacked on top.

| Option | Why not |
|---|---|
| **Zephyr** | The part is **not supported at all**: `soc/ti/` is `{am13, k3, lm3s6965, mspm, simplelink}`. No Tiva SoC, board, pinctrl, clock, PWM or counter driver. The one out-of-tree port is TM4C129x, a different family; the only repo naming TM4C123 has no LICENSE and describes a part with a PWM module this one lacks. Adopting it means writing and owning a SoC port forever. |
| **libopencm3** | LGPL-3.0 with no linking exception — a static MCU image is a §4 Combined Work, so the shipped `.bin` carries relink and anti-tivoization obligations permanently. And `lib/lm4f/` is five files (gpio, rcc, systemcontrol, uart, vector) with **no timer or PWM driver**; `timer.h` is 27 bare offsets with no bit definitions. It solves the problem we already solved and neither that we hadn't. |
| **Linking TivaWare driverlib** | ~1,950 bytes of flash for bring-up we have written, and zero contribution to the burst engine, carrier arithmetic or capture ISR. |
| **TI CMSIS device header** | Non-free: "solely and exclusively on TI's microcontroller products", and explicitly may not be combined with "viral" open-source software. It also has no bit-position defines, so it would not save the error-prone work. |
| **ROM DriverLib** | Held in reserve, not rejected. `rom.h` is BSD-3 and costs no flash, but it is gated on `TARGET_IS_TM4C123_RA1/RA3/RB1` with no part assertion, and call indirection on the carrier-gating path is the wrong trade. Revisit only if flash gets tight. |

## State

| Piece | Status |
|---|---|
| Wire protocol (framing, stuffing, checksum, encode/decode) | **written** |
| Test suite against real captured frames | **MISSING FROM THE TREE** — see below |
| Startup, vector table, linker script (app at 0x1000) | **done** |
| HAL: 50 MHz PLL, UART0 @ 460800, GPIO, µs delay | **done, builds** |
| Board profiles (EA1 + EA3, pins/counts from the stock image) | **done, builds both** |
| Application: opcode dispatch, identity, contacts/relays | **done** (relay/contact state still stubbed to zero) |
| IR transmit **scheduling** (parse, validate, burst schedule) | **written** |
| IR capture **encoding** (bursts -> chunked 0x97 frames) | **written** |
| IR carrier generation (hardware PWM per channel) | **written, untested on hardware** |
| IR transmit loop (burst gating + mid-transmit stop) | **written, untested on hardware** |
| IR receive **demodulator** (carrier-rate edges -> bursts, online) | **written** |
| IR edge capture timer + ISR wiring (the last hardware bit) | **not written** |

> **The host test suite is not in this repository.** There is no `test/`
> directory and no `test` target in the Makefile — `make test` fails with
> "No rule to make target". An earlier revision of this README described a green
> suite replaying real captured frames; whatever produced those results was
> never committed. Until it is restored, treat every "written" row above as
> **unverified**, including the protocol core. The rows are marked accordingly.
> Do not flash anything on the strength of this table alone.

`make fw` produces an image per board, under `build/<board>/`:

```
   text    data     bss     dec     hex
   1445       0    1140    2585     a19  build/ea1/io-mcu.elf
image: 1445 bytes  (app slot is 60 KB at 0x1000)
  vector table: SP=0x20006000 PC=0x00001045
```

The build refuses to call the image flashable unless the vector table is sane
(SP inside SRAM, PC inside the app slot), so a bad link fails loudly instead of
producing something that bricks a part.

**Not yet flashed to real hardware, and not currently covered by any test.**
This is straight-line register code that no one has run on silicon. In
particular the carrier PWM setup and the burst timing are written from the
datasheet and have never driven an LED. Flash this only with the ability to
re-flash the stock image (`/control4/firmware/io/`) and, ideally, hands on the
device.

The carrier is generated by **hardware PWM on each channel's own timer** rather
than by toggling a pin in software, and bursts are gated by handing the pin
between the timer and plain GPIO. That is deliberate: software toggling puts
every interrupt's latency into the waveform, and IR receivers reject a carrier
that wanders. Duty cycle is ~33% rather than 50% — receivers demodulate on
carrier presence, so a shorter on-time gets the same detection for roughly a
third less LED current.

### Burst timing runs on TIMER4

The transmit path needs one timer to time each burst, and it must not be one an
IR channel claims. It used to be **TIMER1 — which is channel 2's carrier
(PF2/T1CCP0) on both EA1 and EA3**, so a transmission on channel 2 would have
reprogrammed the very timer that was timing it. The old comment acknowledged the
clash and deferred it until "the EA1's real channel set is known".

Decoding the vendor's per-board table settled it: across all six board profiles
the outputs use T0–T3 and WT0–WT4, and the receiver uses WT5, which leaves
**TIMER4 and TIMER5 unclaimed on every board**. The burst timer is now TIMER4.

## The protocol core should be testable on its own

The protocol core is deliberately hardware-free so it can be exercised on the
host with no MCU and no cross toolchain. That is the fast loop this design was
built around — and it is currently **not wired up**: there is no `test/`
directory and no `test` target (see the state table above).

Restoring it is the highest-value next step, because the vectors that matter
already exist in the research: real frames captured off a live EA1, recorded in
[docs/io-mcu-firmware.md](../../docs/io-mcu-firmware.md) — `PRODUCT_NAME`,
`UART_RECEIVE`, the `IROUT_SEND` frame that made hardware emit 38 kHz IR, and
the NEC `IRIN_CAPTURED` capture. A suite should assert:

* decode of those captured frames
* encode reproducing the wire bytes exactly
* DLE stuffing round-trips
* survival across every chunk boundary
* rejection of noise, bad checksums, truncation and oversized lengths

Until that exists, "wire-compatible" is an intention, not a demonstrated
property.

## What is left

The host test suite, the two IR engines, and the relay/contact GPIOs.
Everything they need is already pinned down:

* **IR out.** `IROUT_SEND` carries raw Pronto words: word 1 is the carrier
  divisor (`Hz = 4145146 / word`), words 2–3 are once/repeat pair counts, and the
  burst list is in **carrier periods**. Generate the carrier on a timer and gate
  it per burst.
* **IR in.** Capture edges, report as `IRIN_CAPTURED` (0x97): `u16` carrier
  period in timer ticks, then `u16` burst words with bit 15 = carrier on and bits
  0..13 = duration in carrier periods. Long codes are chunked across frames, each
  repeating the carrier header.
* **User serial.** `UART_SET_CONTROL` payload is
  `[port, 1, baud_index, data_bits, parity, stop_bits]`, where baud_index 0..6 is
  1200/2400/4800/9600/19200/38400/57600 and anything else means 115200.
  `UART_SEND` is `[port, data...]`; received bytes go back as `UART_RECEIVE`
  (0xa2) `[port, data...]`. Ports are UART5 and UART7 on both boards, plus
  **UART4 as port 3 on EA3 only** (`UART_USER2_*` in `ir_pins.h`). The app must
  emit one unsolicited `UART_RECEIVE` per port at startup — two frames on EA1,
  three on EA3 — because that is how `ioserver` learns the port count.
* **Relays and contacts.** The vendor table locates the pins: two groups of
  four, `PF0/PF1/PF3/PF4` and `PA2/PA3/PA4/PA5`. EA1 populates none; **EA3
  populates exactly `PF0` and `PA2`** — its one relay and one contact.

  The *protocol* side is confirmed on hardware: `CONTACT_GET` returns a u32
  bitmask, **bit 0 is EA3's contact, and closed reads 1** (verified by shorting
  the input and watching the state toggle both ways). The host **polls** it —
  the MCU never pushes contact changes — so drive it from a read, not an event.

  What is still open is only which *group* is relays and which is contacts.
  Resolve it at first flash: drive the relay pin and listen for the click; if
  it is silent, swap the two constants.

## Build

```bash
make fw                # EA1 image   (needs arm-none-eabi-gcc)
make fw BOARD=ea3      # EA3 image
make all-boards        # both
```

There is no `make test` target yet — see the state table.

## Flashing, when there is something to flash

The TI serial bootloader is reachable today — `overlay/services/ohc-io` already
drives it. Sequence and packet format are in
[docs/io-mcu-firmware.md](../../docs/io-mcu-firmware.md). Because the bootloader
occupies flash `0x0000..0x0FFF` and is never overwritten by an app-slot write, a
bad application image is recoverable by re-running the bootloader handshake and
sending a good one. Keep the vendor images from `/control4/firmware/io/` to hand
as a known-good fallback.
