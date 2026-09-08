# io-mcu — clean-room IO-processor firmware for the HC-800 (LM3S1162)

Replacement firmware for the Control4 IO microcontroller on the HC-800, wire
compatible with the protocol the host already speaks so nothing above it changes.

```sh
make image            # build + wrap in Control4's flashable container
make verify           # check a container's CRC
make verify IMAGE=... # ... including the stock one
```

Needs `arm-none-eabi-gcc`. The part is a **Cortex-M3**, not the EA's M4.

## What is measured, and what is not

This is the important table. Everything in the left column came off real
hardware or out of the vendor binary; everything in the right is still open.

| Established | How |
|---|---|
| Wire protocol, opcodes, checksum | A live HC-800 answered our frames — see below |
| **Host link is 115200** | Same exchange. The EA family runs 460800 |
| Image container + CRC | Decoded from the stock image; `mkimage.py` reproduces its CRC exactly |
| App links at flash `0x1000` | Vendor vector table found there; low 4 KB is the bootloader |
| Carrier is the **PWM module** | Stock vector table claims PWM Fault + generators 0/1/2 |
| **6 IR outputs** | 3 generators x 2 outputs, matching six rear jacks |
| 4 relays, 4 contacts | Panel, and the host's `board.env` |
| **0 user UARTs on the MCU** | `ioserver` holds host `ttyS1`/`ttyS2` directly — see below |

| Still unknown | Consequence |
|---|---|
| **GPIO pin map** for IR / relays / contacts | Outputs are gated off; see `include/ir_pins.h` |
| SRAM size | `lm3s.ld` assumes a conservative 8 KB; firmware reports the truth |
| Crystal frequency | Sidestepped — the clock is measured, not configured |
| IR receiver pin | Capture not implemented |

The live exchange, on `/dev/ttyS3` of a running HC-800:

```
--> 10 02 34 01 00 00 00 cb                    FIRMWARE_VERSION_GET
<-- 10 02 35 01 02 00 08 "03.26.15"        33
--> 10 02 24 02 00 00 00 da                    PRODUCT_NAME
<-- 10 02 25 02 02 00 1b "c4:ir_processor:c4-ir01-i2c" 6c
```

Reply opcode = request + 1, flags bit 1 = response, checksum = negated 8-bit
sum. `ohc_proto.c` — written for the EA — decoded both without modification,
and the reported version matches the extracted image byte for byte.

## Three things that differ from the EA firmware

**The carrier is hardware.** The TM4C1231D5 has no PWM module, so that firmware
synthesises the IR carrier from a timer CCP output. This part has one, and the
stock image uses it. Generator *n* drives outputs PWM(2n) and PWM(2n+1), so
channels 0+1, 2+3 and 4+5 share a counter — and therefore a frequency. That
costs nothing: `IROUT_SEND` carries one carrier for the whole output mask.

**Burst timing cannot be PWM.** Those counters are 16-bit — 1.31 ms at 50 MHz,
while IR gaps run tens of milliseconds. A draft of this used PWM generator 3 and
would have silently truncated every long gap. It is a 32-bit GPTM instead.

**The clock is measured, not set.** Programming the PLL means writing RCC's XTAL
field, which encodes the crystal fitted to the board — unknown, unrecoverable
from the vendor image, and wrong in a way that fails silently rather than
loudly. But the bootloader has already brought the part up and is talking to the
host at a rate we measured, so `lm3s_clock_init()` reads back the UART divisors
it programmed and solves for the system clock:

    clk = baud * 16 * (IBRD + FBRD/64)

No guess anywhere in the chain, and everything downstream is expressed against
a number that is true.

## Safe to flash, deliberately

`OHC_IR_PINS_KNOWN` and `OHC_RELAY_PINS_KNOWN` are both `0`. The firmware runs,
answers the host, and builds correct carriers inside the PWM peripheral — but
never enables an output pin and never drives a relay. **A relay here may be
switching a real load, and guessing a pin to find out is not an acceptable
experiment.** Decode the pin map first, fill in the tables, then flip the flags.

Recovery is the other half of that: the low 4 KB of flash is Control4's serial
bootloader and `mkimage.py` leaves it erased, so a bad application image is
re-flashable over the wire without SWD. Keep a copy of the stock pair from
`/control4/firmware/io/` — that is this MCU's equivalent of the sda2 factory
partition.

## The remaining work

Decode one per-processor config block. The stock image has four at flash
`0x10B8`, stride `0x3D4`, holding TIMER0-3 and UART0-2 bases plus ~41 GPIO
descriptors. The TM4C equivalent turned out to be
`{gpio_base, pin_mask|populated_bit, timer_base, irq_a, irq_b}` at 0x1C stride;
this is likely the same shape with the timer fields replaced by PWM ones. That
one decode unblocks IR output, relays, contacts and capture together.

## Clean room

Written from observed behaviour — frames captured off a live unit, and the
semantics that had to be true for those frames to make sense — plus hardware
facts read out of the vendor image (interrupt vectors, base addresses, container
layout). No vendor code is copied, disassembled into, or translated.
