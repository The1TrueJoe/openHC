---
title: Audio
description: CE5300 I2S into an ADAU1451 SigmaDSP — the I2C bus mainline never creates, and what is still inferred.
sidebar:
  order: 8
---

**Status: kernel drivers written, not yet proven on hardware.** The userspace half
is the HC-800's audio scheme and should work the moment the PCM does. Read
[what is still unproven](#what-is-still-unproven) before trusting any of this.

## The thing that blocked everything

The ADAU1451 codec sits at I²C address `0x38` on the CE5300's **fourth** I²C
controller. Mainline's `i2c-pxa-pci.c` hardcodes `CE4100_PCI_I2C_DEVS 3`, so that
bus is never created and the codec is simply unreachable — `i2cdetect` finds
nothing at `0x38` on any bus, and buses 1 and 2 come up completely empty.

The PCI function really does expose four BARs:

| BAR | Address | Adapter |
|---|---|---|
| 0 | `0xdffe0500` | i2c-0 — lm75 @0x48, RTC @0x68 |
| 1 | `0xdffe0600` | i2c-1 |
| 2 | `0xdffe0700` | i2c-2 |
| 3 | `0xdffe0e00` | **i2c-3 — the codec** |

The fourth address is **discontiguous**, which is why it went unnoticed. Proof it
is the same controller block: on a live EA3 its registers at `+0x14`/`+0x18` read
`0x03`/`0x12`, byte-identical to the three working controllers, with everything
else zero because no driver had configured it. Control4's own ASoC device name
`"adau1451.3-0038"` independently says bus 3, address 0x38.

Fixed in `board/ea/common/patches/linux/0001-i2c-pxa-ce5300-dt.patch`.
**Without that patch nothing else here can work.**

## Layout

```
sound/soc/ce5300/ce5300-i2s.c    platform driver: I2S + scatter-gather DMA (PCI 8086:2e60)
sound/soc/codecs/adau1451-c4.c   codec driver, on the kernel's sigmadsp core
arch/x86/boot/dts/ea-audio-dsp.dtsi  the codec, its firmware, and the card (simple-audio-card)
board/ea/common/firmware/dsp/adau1451/  the DSP's configuration (ea3.json) and its builder
board/ea/common/features/audio-dsp/     kernel config + the firmware package (EA3 DSP)
board/common/features/audio/defconfig   userspace packages (EA + HC-800)
```

Enable with `audio` and `audio-dsp` in a board's `ohc.features`. There is no
board machine driver: the card is the kernel's `simple-audio-card`, described in
the DTB.

## No vendor binaries are required

Two separate things were assumed to need signed firmware. Neither does, for
playback:

- **The SoC audio DSPs** run 1.8 MB of signed firmware loaded through the security
  engine, but they only do decode and mix. The **I²S render path needs none of
  it**, which is what makes a mainline driver possible at all.
- **The ADAU1451** boots its own program from the board's EEPROM; Linux only
  pushes parameter RAM. Stock requests `ea3-1451.bin`, but at 48 kHz nothing
  from it is ever written. openHC loads a firmware file of **its own** instead,
  built from `ea3.json`: the kernel's stock sigmadsp core writes its tables and
  makes its knobs into ALSA controls. See [the DSP](#the-dsp-openhcs-configuration).

:::caution[One header must never be committed]
`ea3-dsp-params.h` is 10,410 `#define`s with an Analog Devices header stating it
"may not be distributed whole or in any part to third parties". openHC does not
need it: the addresses its tone controls use were read off a running unit, and
every table they index is computed by openHC.
:::

## The DSP: openHC's configuration

The program in the EEPROM reads every knob as an **index 0..48** from a
parameter cell and looks the setting up in a 49-entry table, 24 neutral. openHC
fills those tables from its own designs and hands the cells to ALSA:

| Knob | Design |
|---|---|
| Bass, Treble | RBJ shelves at 120 Hz / 8 kHz, ±12 dB — the HC-800's software tone stage |
| Balance | linear, one side to silence — the HC-800's |
| EQ 31 Hz … 16 kHz | ten RBJ peaking bands, Q 1.414, ±12 dB |

`board/ea/common/firmware/dsp/adau1451/ea3.json` is the whole configuration —
cells, tables, designs — and `mkfw.py` builds it into the kernel's SigmaDSP
firmware format (`/lib/firmware/openhc/adau1451-ea3.bin`). The board DTB names
the file and the program it belongs to (`control4,program-version`); on any
other program the driver writes nothing. ohc-audiod reads the same profile for
its knob list, so the web UI shows exactly those knobs, set over MQTT like the
HC-800's tone. The [directory's README](https://github.com/The1TrueJoe/openHC/tree/main/board/ea/common/firmware/dsp/adau1451)
has the map and how it was proven.

## What the stock system exposes

Two ALSA cards on a stock EA3. Card 0 is the SoC's Intel SMD audio; card 1 is the
SigmaDSP.

```
card 0 [IntelCE353xx]
  device 0  analog0    -> line out (via AK4621)
  device 1  digital0   -> S/PDIF coax out
  device 2  hdmi0      -> HDMI audio
card 1 [ninjagosmdadau1  "ninjago-smd-adau1451"]
  device 0  SMD DAI PCM adau1451-ch0-0
```

**No capture devices at all.** The line-in and coax-in jacks aren't reachable
through ALSA on the stock image. They're wired into the ADAU1451, which mixes
them internally under DSP program control. Anything wanting them as a capture
source has to go through the DSP.

Card 1's mixer controls are a full 10-band graphic EQ plus tone and loudness —
that's the ADAU1451 program, not a generic codec.

The fixed output preferences, from `/etc/hdmi_hpd.cfg`: S/PDIF at 96 kHz /
24-bit ("optical and coaxial spdif are the same output"), HDMI PCM plus encoded
DTS/DD/AAC up to 8ch, and `i2s_audio_pref enabled=2`, i2s1 only, the SoC↔ADAU
link.

## Where the register knowledge came from

`ismdaudio.ko` shipped with DWARF debug info, so Ghidra recovers original source
filenames and struct layouts. The vendor HAL writes registers through a helper
taking repeated 5-tuples of `(reg_offset, bit_pos, bit_width, bit_mask, value)`.

**Every observed tuple satisfies `bit_mask == ((1 << bit_width) - 1) << bit_pos`**
— nine fields in one call, six in another, six in a third, with no exceptions.
That redundancy is what makes the decode trustworthy rather than a guess: a wrong
field order breaks the invariant immediately.

Two results worth repeating because they are structural rather than guessed:

- **Bit 28 of the DMA control word is "halt after this descriptor."** The ring is
  chained once and never re-linked; queueing a buffer marks the new tail with
  STOP and clears STOP on the one before it, so the engine can never run into a
  stale descriptor.
- **`[2:1]` is the destination address mode and `[6:5]` the source.** Render puts
  the fixed FIFO port on the destination side and capture on the source side, and
  the field carrying `FIXED_CONTINOUS` follows it exactly. Two independent
  datapaths agreeing.

Full extraction: the [CE5300 audio register map](/ea/audio-regmap/).

## What is still unproven

The platform driver **does not register a PCM by default** (`enable_pcm=0`). It
binds, maps BAR0 and dumps registers, and registers its DAI with a startup that
refuses — so the card forms and the DSP controls can be tuned, while anything
opening the PCM gets `-ENODEV` at once. That is deliberate: registering a PCM whose
trigger cannot reliably move samples is worse than absent, because userspace
opens it, gets silence, and the failure looks like a codec or routing problem
somewhere else entirely.

A stock unit was read while it played (2026-10-26; the
[register map](/ea/audio-regmap/#measured-on-a-stock-unit-while-it-plays) has
the capture), and the driver now follows it: the base, the descriptor format and
its mode word, the DMA context's registers — and with them **the playback
position** (`+0x1010`, the current source address), which `.pointer` reads — and
TX0's configuration. Periods are paced by a timer from that position, so no
interrupt is needed. Two things are still inferred:

1. **Which writes start the engine.** The vendor's three ("descriptor base,
   size, mode") are taken as the head descriptor into `NEXT_DESC`, 0 into the
   bytes-left register, and the mode word with bit 31 (set while stock plays,
   never in a descriptor) into the control register; the TX run bit as bit 0 of
   TX0 `+0x00`, the register that reads `0x21` while stock plays (bit 0 of
   `TX_CTRL`, the old guess, reads 0 there).
2. **The ungate** — why the TX and DMA blocks hang on access under openHC when
   stock reads them freely. The plan is below.

Master/slave couldn't be determined either, but it matters less than it sounds:
the render path exposes **no** clock-direction control at all, consistent with the
SoC side being permanently slave, and the codec is I²S master. So don't program
the bit-clock divider speculatively.

:::danger[Do not poke this device with devmem]
An unbound PCI function may have memory decode off or sub-blocks unclocked.
Doing exactly that at offsets `0xc000`/`0xf000` **hung an EA3 hard** and cost a
power cycle. The driver's probe-time dump exists precisely so those reads happen
with the device already enabled.
:::

## Bring-up order

1. Flash a kernel with the i2c patch; confirm `i2cdetect -y -r 3` shows `0x38`.
   **Nothing below matters until this passes.**
2. With the card up (`aplay -l` shows `openhcea`), `amixer -c openhcea contents`
   lists the DSP controls and `ReadBack DSP Program` reads 10: the DSP is alive
   and running openHC's parameters.
3. **Find the ungate** — reads first, then one write at a time. Every write
   below is a test: do it on a **RAM-booted** openHC (`ohc.ramboot=1`), never a
   flashed kernel, and expect to power-cycle if a read hangs.
   1. Read the Clock and Reset Unit (`00:00.2`, registers at `0xbfffef00`) and
      compare with stock's values in the register map:

      ```sh
      for o in $(seq 0 4 156); do printf '%02x %s\n' $o $(devmem $((0xbfffef00 + o)) 32); done
      ```

      Every register that differs is a candidate clock-enable or reset bit.
   2. Read `AUD_IO +0x00` (`devmem 0xdfa80000 32`, proven safe): `0x1` here,
      `0x7` on stock. Its bits 1–2 are the first suspects — the block's own
      sub-block enables.
   3. Try the cheapest first: set `AUD_IO +0x00` to `0x7`, then read one TX
      register (`devmem 0xdfa82000 32`). If that hangs or answers nothing, set
      the CRU candidates one register at a time, re-testing the same read.
   4. Whatever turns out to ungate it goes into the driver's probe (or into the
      board DT if it is a clock the kernel can name), and `dump_regs=1` then
      shows the TX/DMA blocks the way the stock capture does.
4. `ce5300_i2s.enable_pcm=1`, then `speaker-test -c2 -twav`. The position should
   advance (`/proc/asound/openhcea/pcm0p/sub0/status`), the DAC unmutes while it
   plays, and the DSP's Mode → Test tone is the way to tell a dead I²S from a
   dead analog path.

## AirPlay and Spotify Connect

Both are plain ALSA clients and need no special kernel support beyond a working
card.

**AirPlay** is `shairport-sync` 4.3.7, announced via Avahi over D-Bus. It is
built as classic AirPlay (AirPlay 1). Buildroot 2026.02 can build AirPlay 2
(nqptp), but AirPlay 2 wants one receiver per host, and the HC-800 runs several
named AirPlay endpoints on one box, each with its own device ID. Classic
AirPlay works from every Apple device; it just can't join AirPlay-2 multi-room
groups.

**Spotify Connect** is `librespot`, packaged here because Buildroot has none. It
is built with Buildroot's cargo infrastructure rather than the host-side route
the other openHC Rust daemons use, because it links against the target's
alsa-lib. It **requires a Spotify Premium account**, with a free account the
device is discovered and then refuses to play, which looks like a bug and isn't
one.

## What still blocks sound

The `audio` and `audio-dsp` features are on for both EA3 boards and the drivers
build for x86_64 (modules, loaded by `S45ea-audio`). Two things stand between
that and sound:

- **The DSP has no program.** The ADAU1451 does not boot one by itself:
  measured under openHC, its PLL is off, its core idle and its RAM random. Stock
  loads everything over I²C at boot from a userspace SigmaStudio download
  (`/control4/bin/ea3_dsploader`). openHC's own program is being built with
  SigmaStudio+ in CI, and the driver gains a load path for it.
- **The CE5300 I²S block hangs on access.** Reading its TX/DMA registers
  (`0x2004` and beyond) hangs the SoC under openHC, where stock reads them
  freely. The register dump stays off until the cause below is found.

## The CE5300's power and clock controllers

The obvious explanations for that hang are a powered-down island or a gated
clock. Both were checked against a stock unit, **read-only**, and both are
ruled out for audio. The work also mapped the controllers, which the
[switch bring-up](/ea/switch/) leaned on too.

### P-Unit power islands

The SoC switches whole functional blocks on and off through its P-Unit. Stock
drives it from Intel's closed `intel_ce_pm.ko`; disassembled, its `punit_mode()`
is a short port-I/O handshake:

| | |
|---|---|
| Device | PCI `00:1e.0` (`8086:070c`); the OSPM registers are its I/O BAR1, `0xb040` on the units seen |
| Island word | `+0x20`: two bits per island, set = powered **off**; **audio is island 4, mask `0x0c00`** |
| Command | `+0x08` &= ~`0x30`; write the word to `+0x20`; `+0x04` = (old & `0xfe000000`) \| (seq << 21) \| `0x201` |
| Done | poll `+0x00` until bit 8 (busy) clears and bits 12:9 echo `seq` |

`/proc/icepm/state` on stock lists the islands and their drivers: 3 holds
`ismdclock`, GbE, PCIe, SATA and USB; **4 holds `audio` alone**; 5–9 are video
decode, mux/encode, post-processing, capture/display and graphics. The island
word reads `0` on stock **and on openHC**: every island is powered, so this is
not the gate.

### Clock and Reset Unit

PCI `00:00.2` (`8086:2e52`), BAR0 `0xbfffef00`, 256 bytes. Stock drives it from
Intel's closed `clock_control.ko`, whose table names 185 fields (register, bit,
width). The audio ones:

| Register | Bits | Field |
|---|---|---|
| `+0x0c` | 13, 12 | `AU_DSP1_CLK_EN`, `AU_DSP0_CLK_EN` |
| | 11:6 | `DSP1_CLK_FRQ`, `DSP0_CLK_FRQ` |
| | 1, 0 | `AU_DSP1_RST`, `AU_DSP0_RST` |
| `+0x10` | 4 | `ADAC_RESET` |
| | 3, 2 | `MCLK1_OUT_EN`, `MCLK0_OUT_EN` |
| | 1, 0 | `AU_XSI_CLK_EN`, `AU_IF_RST` |
| `+0x7c` | 31:0 | `AV_DDS_CTRL` |

Stock and openHC read **identically** here (`+0x0c` = `0xf843`, `+0x10` =
`0x17`: interface out of reset, XSI clock on, MCLK0 out on), so audio is not
gated here either. The whole block differs in two registers only: `+0x94`
(`0x7ff` on openHC, `0x7fc` on stock) and `+0x98` (`0x7ff` vs `0x7fe`).
Neither is named in `clock_control`'s table, and it names no SPI/SSP clock at
all.

### The A/V media clock

PCI `01:0c.1` (`8086:2e6f`), BAR0 `0xdffe1000`, bound by stock's `ismdclock`.
It holds the media timestamp and clock-recovery counters: on stock they tick
even with nothing playing, in groups at a `0x40` stride. Under openHC the whole
block reads zero; nothing initializes it. It gates no peripheral clock.

### What is left

- **`AUD_IO +0x00`** is the one audio register that differs: `0x1` under
  openHC, `0x7` on a playing stock unit. Its bits 1–2 are the prime suspects
  for the I²S block's own sub-block enables, and the next test is to set them
  and read one TX register (`0xdfa82000`), on a RAM-booted openHC, expecting a
  possible hang.
- **The master clock's rate.** The ADAU1451 runs from the SoC's MCLK0 (enabled
  on both). Its rate may come from the A/V DDS or audio PLL that stock's media
  stack programs, and that stack never runs under openHC. If the DSP's PLL
  will not lock once a program is loaded, this is where to look.

The stock unit's I²S/DMA registers, captured while it played, are in the
[register map](/ea/audio-regmap/#measured-on-a-stock-unit-while-it-plays).
