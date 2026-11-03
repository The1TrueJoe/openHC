---
title: Secondary firmware
description: The auxiliary processors beside the main SoC, and how the tree is organised around them.
sidebar:
  order: 6
---

The main SoC isn't the only processor on these boards. Each controller carries
one or more auxiliary chips running their own firmware, and they differ by model.

```
board/common/firmware/
  io-mcu/              EA1 / EA3 / EA5 + HC800 — TI Tiva TM4C1231D5 (M4) and
                       Stellaris LM3S1162 (M3), one image per part  (implemented)
  zigbee/em357/        every board except the IOX                  (host-side tools)
board/ea/common/firmware/
  dsp/adau1451/        EA3 / EA5        — Analog Devices SigmaDSP
```

Firmware lives under the board tree that owns it. The IO microcontroller's and
the Zigbee radio's sit in `board/common`, because more than one family carries
the part.

## One source tree, two parts

The IO MCU speaks the same host protocol — [DLE/STX over a
UART](/shared/io-mcu/) — across the whole line, but the silicon underneath isn't
the same part: a Cortex-M4 on the EA family, a Cortex-M3 on the HC-800. So the
tree is a hardware-free core (protocol, pin-table checks, IR engine, the
application) plus a HAL per part, and the constants it shares with the kernel
driver live in one header beside the driver. Neither image carries a board: the
driver tells it the pins at run time. See [IO-MCU firmware](/shared/io-mcu-firmware/).

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

`io-mcu`: both images build in every image with the `iomcu` feature and pass the
host tests; none has run on a part yet, and the driver writes one only to a board
whose node names it. The DSP has its own program and parameter file (see
[audio](/ea/audio/)).

## The DSP and the Zigbee radio, on redistribution

Neither needs an image shipped.

**The ADAU1451** self-boots its own program from EEPROM. The vendor's loader only
pushes parameter RAM, and at 48 kHz it writes nothing at all, so openHC's driver
drops `request_firmware` entirely. See [audio](/ea/audio/).

**The EM357** already has its NCP application flashed on every unit's radio. Our
software only tells the existing image to *run* — bootloader option `2`. The `.ebl`
stays on the user's own device. See
[the EA1's Zigbee section](/ea/ea1/#zigbee-the-bootloader-trap).
