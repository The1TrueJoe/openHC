---
title: EA-5
description: The EA-5 ("garmadon") — what it adds over an EA-3, the audio FPGA, and what openHC does with it so far.
sidebar:
  order: 3.5
---

The EA-5 is the EA-3's platform (CE5300, CEFDK, TM4C IO-MCU, EM357 Zigbee) with
a bigger IO panel and a different audio section. Board: **GARMADON MAIN BOARD,
100-00257 rev C**. openHC builds it as two boards: `ea5-v1` (internal
Wi-Fi) and `ea5-v2` (no radio). Everything else is the EA-5 model
(`board/ea/ea5/`).

**Nothing here has run on an EA-5 yet.** Everything below comes from Control4's
GPL kernel (OS 3.3.1), the stock root filesystem (shared by every EA), the
installation guide and photos of the board. The [bring-up](#bring-up) work list says
what to confirm first.

## Panel

| | |
|---|---|
| IR | 8 jacks + front blaster + front receiver. **IR 1 and 2 are also serial 3 and 4** |
| Serial | SERIAL 1/2: two DB9s with flow control — not on the IO-MCU |
| Relays / contacts | 4 / 4 |
| Network | ETHERNET IN + 4-port gigabit switch (BCM53125) |
| Audio in | 2 stereo analog (AK5388), 2 coax S/PDIF |
| Audio out | 2 stereo analog (AK4490 ×2), 2 coax S/PDIF, HDMI |
| Storage | 1 USB, 1 eSATA |
| Wi-Fi | V1: AR9462 mini-PCIe (ath9k). V2: optional USB adapter |

## GPIO

`setup_gpio_ea5()` in stock `c4_gpio_config` adds only `usb_3_reset` (28, high)
to the common set; it drops the EA-3's `poe_type` and `ac_pwr`. `n_fpga_reload`
(57) stays high: low reloads the FPGA from its PROM. At shutdown stock mutes the
DACs through the FPGA (control register bit 17), not GPIO 61.

## The audio FPGA

A Xilinx **Spartan-6 XC6SLX45T** on PCIe (`02:00.0`, `10ee:0007`). It configures
itself from its own PROM at power-on: an XCF16P on board revisions ≤ 4 (stock
reprograms it over JTAG from `garmadon-fpga-45t-rev7.xsvf`), SPI flash after that
(`garmadon-fpga-45t-spi-rev8.bin`, written by a package postinst). **openHC never
writes either**, so it ships no bitstream.

What it does:

- **DMA**: four stereo pairs each way between host memory and the FPGA. Per
  pair and direction the host gives it one block's address; it moves the block,
  interrupts, and takes the next address.
- **Matrix**: 32 outputs × 32 inputs of 16-bit gains, 0.1 dB a step,
  `0x30F` = 0 dB, 0 = off.
- **S/PDIF**: it receives and transmits both coax pairs itself.
- **DACs**: soft mute (bit 17) and power-down (bit 14) of the control register.

Registers (BAR0), from the GPL `ninjago_fpga.c`:

| Offset | |
|---|---|
| `0x00` | revision (`0xffff`: PROM not programmed) |
| `0x04` | status |
| `0x08` | control: bit 17 DAC soft mute, bit 14 DAC power-down |
| `0x0c` | buffer control: 8 bits per pair, playback `[3:0]`, capture `[7:4]`; bits 1:0 sample size (1/2/3 = 16/24/32), bit 2 start |
| `0x10` | IRQ enable: capture pair *p* = bit *p*, playback = bit *p*+8 |
| `0x14` | IRQ status (write-1-to-clear up to rev 4, auto-clear after) |
| `0x30`–`0x40` | debug counters |
| `0x84` / `0xa4` | capture / playback block size, in dwords |
| `0x90 + 4p` / `0xb0 + 4p` | capture / playback DMA address for pair *p* |

BAR1 is the FPGA's own buffer (its length / 4 / 4 blocks is the block size,
trimmed to a multiple of 24 bytes); BAR2 is the matrix, `(32·out + in)·2`.

Matrix ports:

| Index | Input | Output |
|---|---|---|
| 0–7 | host playback pairs 0–3 | host capture pairs 0–3 |
| 8–15 | ADC 1, ADC 2, S/PDIF in 1, S/PDIF in 2 | DSP 1 inputs 1–4 |
| 16–23 | DSP 1 outputs 1–2, DSP 2 outputs 1–2 | DSP 2 inputs 1–4 |
| 24–27 | — | DAC 1, DAC 2 |
| 28–31 | — | S/PDIF out 1, S/PDIF out 2 |

Stock routes every output through a DSP: each ADAU1451 runs two chains, the
first to its DAC and the second to its S/PDIF out, fed from its ADC, its S/PDIF
in and two host pairs.

## The DSPs

Two ADAU1451s on i2c-3 at `0x38` and `0x39` (stock `adau1451x2`), one per zone,
both running one program (`ea5-1451.bin`, "Garmadon Rev10 DSP 48K"). It is not
the EA-3's: two complete output chains per chip (volume, tone, loudness,
balance, 10-band EQ, each twice), no input mixer (the FPGA does that), and a
DSP bypass. The tone and EQ coefficient tables are byte-identical to the EA-3's.

The register init, from SigmaStudio's defaults compiled into stock
`ea5_dsploader`:

| Register | Value | |
|---|---|---|
| `PLL_CTRL0/1`, `PLL_CLK_SRC` | `0x60`, `0x03`, `0x01` | as the EA-3: 24.576 MHz MCLK → 294.912 MHz |
| `MCLK_OUT` | `0x0006` | off |
| `POWER_ENABLE0` | `0x1fff` | |
| `ASRC_MUTE` | `0x0400` | every ASRC open (the EA-3 mutes 1–7) |
| `CLK_GEN2_M`, `CLK_GEN3_M/N` | `6`, `5`/`1` | |
| `MP6_MODE`, `MP7_MODE`, `MP7_WRITE` | `5`, `5`, `1` | |
| `ASRC_INPUT0`, `ASRC_INPUT7` | `0xa1` | SDATA_IN3 ch 40/41 |
| `ASRC_OUT_RATE0/7` | `1` | |
| `SOUT_SOURCE0–23` | `2` | |
| `SERIAL_BYTE_0_0`, `_1_0` | `0x0001` | inputs 0 and 1 in TDM (the FPGA's pairs) |
| `SERIAL_BYTE_3_0` | `0x6c00` | as the EA-3 |
| `SERIAL_BYTE_4_0`–`_7_0` | `0x0400` | the four outputs |
| `START_PULSE` | `8` | |

No EA-5 DSP program exists in openHC yet, so the DSPs are left as they come up.

## What openHC does

- **Card `openhcea5`** (`snd-soc` driver `ea5-fpga`): one PCM per FPGA pair, each
  playback and capture, 48 kHz, S16/S24_3LE/S32. The PCMs are named in the
  device tree (`control4,link-names`), `Audio 1` to `Audio 4` until each jack is
  checked; rename them in `ea-audio-fpga.dtsi` as they are. The DSPs are on the card as aux
  devices, controls prefixed `Zone 1` / `Zone 2`.
- **Routing, DSPs bypassed**: PCM 0 analog 1, PCM 1 coax 1, PCM 2 analog 2,
  PCM 3 coax 2, in and out. The DSP inputs get stock's routing, so a program can
  be switched in later.
- **`Matrix N Volume`**: one control per matrix output, 32 values (one per
  input), so any route can be changed with `amixer`.
- **DAC mute** follows playback: unmuted while any playback runs.
- `/sys/kernel/debug/ea5-fpga` dumps the registers.
- **Switch**: `lan0` to `lan4` in `ea5.dtsi`; which port is which jack is
  unverified.
- **Combo jacks**: IR 1 / Serial 3 and IR 2 / Serial 4, see
  [combo jacks](/shared/io-mcu/#combo-jacks).

Not done: an EA-5 DSP program, the DB9 serial ports, sample rates other than
48 kHz, the port-to-jack map.

## Bring-up

The work list (identity and boot, the audio FPGA, the DSPs, the switch, the
IO-MCU, the DB9 serial ports and the rest) is
[issue #128](https://github.com/The1TrueJoe/openHC/issues/128). Netboot first:
it writes nothing.
