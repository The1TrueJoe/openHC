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

Four things are still inferred:

1. **Which three DMA registers `render_start` writes.** `__regparm3` stripped the
   arguments. Head → `0x1010`, tail → `0x1038`, control → `0x101c` is the only fit
   matching the two descriptor addresses the render context carries.
2. **Which bit of `TX_CTRL` is the stream enable.** Bit 0 is the only 1-bit field
   ever written there and channel-config clears it, so "cleared to reconfigure,
   set to run" is the natural reading, but no site proves it.
3. **The interrupt mask register offset.** The read-modify-write is visible; the
   offset isn't.
4. **The DMA position register**, which is why `.pointer` currently returns 0.
   ALSA needs a real byte position; fabricating one makes playback appear to work
   while drifting.

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
2. Load the codec alone and read its samplerate register, it should report
   48000, proving the DSP is alive and running its own program.
3. Bring up the card so `aplay -l` lists it and `amixer` works.
4. Confirm the four inferred fields against the register dump, set
   `enable_pcm=1`, and finish `.pointer`.
5. `speaker-test -c2 -twav`. This is what distinguishes "no card" from "no
   sound".

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

## Why the feature is currently off

`audio` is disabled on ea3-v2 to keep the image under the
[copy window](/ea/boot-chain/#the-copy-window). The drivers compile on
i686 but have never been built for x86_64, and the platform driver is probe-only
anyway, so right now the feature can contribute nothing except size and compile
risk.

It has a second blocker regardless of size: **`ce5300-i2s.c` reading TX register
`0x2004` hangs the SoC**, so the register dump must stay off.
