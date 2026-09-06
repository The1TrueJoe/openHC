# EA-series audio: CE5300 I2S → ADAU1451 → analog out

Status: **kernel drivers written, not yet proven on hardware.** The userspace
half (ALSA, AirPlay, Spotify Connect) is standard and should work the moment a
card appears. Read the "What is still unproven" section before trusting any of
this.

## The thing that blocked everything

The ADAU1451 codec sits at I2C address 0x38 on the CE5300's **fourth** I2C
controller. Mainline's `i2c-pxa-pci.c` hardcodes `CE4100_PCI_I2C_DEVS 3`, so
that bus is never created and the codec is simply unreachable — `i2cdetect`
finds nothing at 0x38 on any bus, and buses 1 and 2 come up completely empty.

The PCI function (8086:2e68 at 01:0b.2) really does expose four BARs:

| BAR | address | adapter |
|---|---|---|
| 0 | `0xdffe0500` | i2c-0 — lm75 @0x48, RTC @0x68 |
| 1 | `0xdffe0600` | i2c-1 |
| 2 | `0xdffe0700` | i2c-2 |
| 3 | `0xdffe0e00` | **i2c-3 — the codec** |

The fourth address is discontiguous, which is why it went unnoticed. Proof it is
the same controller block: on a live EA3 its registers at `+0x14`/`+0x18` read
`0x03`/`0x12`, byte-identical to the three working controllers, with everything
else zero because no driver had configured it. Control4's own ASoC device name
`"adau1451.3-0038"` independently says bus 3, address 0x38.

Fixed in `board/ea-common/patches/linux/0001-i2c-pxa-pci-enumerate-without-DT.patch`.
**Without that patch nothing else here can work.**

## Layout

```
sound/soc/ce5300/ce5300-i2s.c    platform driver: I2S + scatter-gather DMA (PCI 8086:2e60)
sound/soc/ce5300/ce5300-ea3.c    machine driver: ties CPU DAI + codec into a card
sound/soc/codecs/adau1451-c4.c   codec driver
board/ea-common/linux/audio.fragment      kernel config
board/ea-common/features/audio.defconfig  userspace packages
```

Enable with `audio` in a board's `ohc.features`.

## No vendor binaries are required

Two separate things were assumed to need signed firmware. Neither does, for
playback:

* **The SoC audio DSPs** (8086:2e5f) run 1.8 MB of signed firmware loaded through
  the security engine — but they only do decode/mix. The **I2S render path needs
  none of it**, which is what makes a mainline driver possible.
* **The ADAU1451** requests `ea3-1451.bin`, but at 48 kHz nothing from it is ever
  written: the vendor pre-seeds the DSP's current samplerate to 48000, its
  `sigmadsp_reload()` returns immediately, and the data-chunk loader is `#if 0`'d
  dead code. The codec self-boots its own program from EEPROM; Linux only pushes
  parameter RAM. So `request_firmware` is dropped entirely.

**`ea3-dsp-params.h` must never be committed.** It is 10,410 `#define`s with an
Analog Devices header stating it "may not be distributed whole or in any part to
third parties". The vendor driver uses 41 of those symbols and every one is a
mixer control — bass, balance, loudness, input gain, a 10-band EQ. None are in
the audio datapath, so the port omits all of them. Restoring tone controls later
needs exactly those 41 addresses, not the file.

## Where the register knowledge came from

`ismdaudio.ko` shipped with DWARF debug info, so Ghidra recovers original source
filenames and struct layouts. The vendor HAL writes registers through a helper
taking repeated 5-tuples of `(reg_offset, bit_pos, bit_width, bit_mask, value)`.
Every observed tuple satisfies `bit_mask == ((1 << bit_width) - 1) << bit_pos`,
and that redundancy is what makes the decode trustworthy rather than a guess —
a wrong field order breaks the invariant immediately.

Two results worth repeating because they are structural, not guessed:

* **Bit 28 of the DMA control word is "halt after this descriptor."** The ring is
  chained once and never re-linked; queueing a buffer marks the new tail with
  STOP and clears STOP on the one before it, so the engine can never run into a
  stale descriptor. Render and capture flip only this bit between their two
  otherwise identical flag words.
* **`[2:1]` is the destination address mode and `[6:5]` the source.** Render puts
  the fixed FIFO port on the destination side and capture on the source side, and
  the field carrying `FIXED_CONTINOUS` follows it exactly. Two independent
  datapaths agreeing.

Full working notes: `scratchpad/audio-re/REGISTER-BITS.md`.

## What is still unproven

The platform driver **does not register a PCM by default** (`enable_pcm=0`). It
binds, maps BAR0 and dumps registers. That is deliberate: registering a PCM whose
trigger cannot reliably move samples is worse than absent, because userspace
opens it, gets silence, and the failure looks like a codec or routing problem
somewhere else entirely.

Still inferred, all marked in `ce5300-i2s.h`:

1. **Which three DMA registers `render_start` writes.** `__regparm3` stripped the
   arguments. Head → `0x1010`, tail → `0x1038`, control → `0x101c` is the only
   fit that matches the two descriptor addresses the render context carries.
2. **Which bit of `TX_CTRL` is the stream enable.** Bit 0 is the only 1-bit field
   ever written there and channel-config clears it, so "cleared to reconfigure,
   set to run" is the natural reading — but no site proves it.
3. **The interrupt mask register offset.** The read-modify-write is visible; the
   offset is not.
4. **The DMA position register**, which is why `.pointer` currently returns 0.
   ALSA needs a real byte position; fabricating one makes playback appear to work
   while drifting.

Master/slave could not be determined either, but it matters less than it sounds:
the render path exposes **no** clock-direction control at all, consistent with
the SoC side being permanently slave, and the codec is I2S master (`CBM_CFM`).
So do not program the bit-clock divider speculatively.

## Reading these registers safely

Do **not** poke this device with `devmem` while no driver is bound. An unbound
PCI function may have memory decode off or sub-blocks unclocked; doing exactly
that at offsets `0xc000`/`0xf000` hung an EA3 hard and cost a power cycle. The
driver's probe-time dump exists precisely so those reads happen with
`pcim_enable_device()` already done.

## Bring-up order

1. Flash a kernel with the i2c patch; confirm `i2cdetect -y -r 3` shows `0x38`.
   Nothing below matters until this passes.
2. Load the codec alone and read its samplerate register — it should report
   48000, proving the DSP is alive and running its own program.
3. Bring up the card so `aplay -l` lists it and `amixer` works.
4. Confirm the four inferred fields above against the register dump, set
   `enable_pcm=1`, and finish `.pointer`.
5. `speaker-test -c2 -twav` — this is what distinguishes "no card" from "no
   sound".

## AirPlay and Spotify Connect

Both are plain ALSA clients and need no special kernel support beyond a working
card.

* **AirPlay** — `shairport-sync`, announced via Avahi. This is AirPlay **1**:
  AirPlay 2 needs `nqptp`, which Buildroot 2024.02 does not package. AirPlay 1
  works from every Apple device; it just cannot join multi-room groups.
* **Spotify Connect** — `librespot` (`packages/librespot`, openHC's own package;
  Buildroot has none). Built with Buildroot's cargo infrastructure rather than
  the host-side route the other openHC Rust daemons use, because it links against
  the target's alsa-lib. **Requires a Spotify Premium account** — with a free
  account the device is discovered and then refuses to play, which looks like a
  bug and is not one.
