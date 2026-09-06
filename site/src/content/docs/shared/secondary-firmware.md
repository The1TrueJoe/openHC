---
title: Secondary firmware
description: The auxiliary processors beside the main SoC, and how the tree is organised around them.
sidebar:
  order: 4
---

The main SoC isn't the only processor on these boards. Each controller carries
one or more auxiliary chips running their own firmware, and they differ by model.

```
board/ea-common/firmware/
  io-mcu/tm4c1231d5/   EA1 / EA3 / EA5  — TI Tiva, Cortex-M4F      (implemented)
  dsp/adau1451/        EA3 / EA5        — Analog Devices SigmaDSP  (placeholder)
board/hc800/firmware/
  io-mcu/lm3s1162/     HC800 / HC250    — TI Stellaris, Cortex-M3  (placeholder)
board/common/firmware/
  zigbee/em357/        every board except the IOX                  (host-side tools)
```

Firmware lives under the board tree that owns it, alongside everything else that
is board-specific. **Zigbee is the exception** and sits in `board/common`, because
every controller except the IO Extender carries an EM357.

## The caveat this layout accepts

The IO MCU speaks the same host protocol — [DLE/STX over a
UART](/shared/io-mcu/) — across the whole line, but **the silicon
underneath isn't the same part**: a Cortex-M4F on the EA family, a Cortex-M3 on
the HC family. The linker script, startup code and peripheral map differ and
can't share a build.

Keeping each MCU in its own directory lets the shared protocol live in one place
while the hardware layer stays honest about which chip it targets.

The trade is real and worth naming: if the LM3S1162 is ever implemented for the HC
family it will need `ohc_proto.h`, which now lives in the EA tree. Copy it or
factor it out at that point — **do not let the two drift**, because the whole
reason the host side needs no changes is that both speak the identical protocol.

## Relationship to the main firmware

None of this is the Linux image. The kernel and rootfs build produces what runs on
the main SoC. This tree is the code that runs *beside* it on the auxiliary
processors, flashed over their own links:

| Processor | Flashed over |
|---|---|
| IO MCU | the TI serial bootloader, on the host UART |
| ADAU1451 DSP | I²C |
| DM355 FPGA | Xilinx slave-serial bit-bang over seven GPIOs |
| Zigbee NCP | the EM357 serial bootloader, ZMODEM upload |

## What is implemented

Only `io-mcu/tm4c1231d5`, the EA family's clean-room replacement. The other
directories are stubs that record what a given board needs, so the structure is
ready when those boards are.

The board profile for the Tiva firmware is currently **compile-time**, selected in
`board_profile.h`. It could be runtime-selected the way the vendor does it, from
the [ADC board-ID strap](/shared/io-mcu/#the-selector-disassembled), the
arithmetic is fully decoded and the constants are already in that header. What
stops it is that the predicted strap voltages have never been measured.

## The DSP and the Zigbee radio, on redistribution

Neither needs an image shipped.

**The ADAU1451** self-boots its own program from EEPROM. The vendor's loader only
pushes parameter RAM, and at 48 kHz it writes nothing at all, so openHC's driver
drops `request_firmware` entirely. See [audio](/ea/audio/).

**The EM357** already has its NCP application flashed on every unit's radio. Our
software only tells the existing image to *run* — bootloader option `2`. The `.ebl`
stays on the user's own device. See
[the EA1's Zigbee section](/ea/ea1/#zigbee-the-bootloader-trap).
