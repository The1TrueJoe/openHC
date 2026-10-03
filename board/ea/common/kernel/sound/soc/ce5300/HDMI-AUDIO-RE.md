<!-- SPDX-License-Identifier: GPL-2.0 -->
# CE5300 HDMI audio — reverse-engineering notes

Status: **skeleton only, no working backend.** `ce5300-hdmi.c`,
`ce5300-ea1.c` and `ce5300-ea1-board.c` register a valid but **no-op** ASoC card
so EA1 enumerates an HDMI PCM device; none of them touch a hardware register.
This file records what is known from source, what is still un-RE'd, and the
**supervised** plan to finish it.

> **DANGER — why this is supervised-only.** Reading an un-ungated CE5300 audio
> TX/DMA register **hangs the SoC dead** (power cycle required, no software
> recovery). `ce5300-i2s.c` wedged an EA3 reading its TX block at `0x2004` even
> with the PCI function enabled; poking `0xc000`/`0xf000` with `devmem` hung an
> EA3 just as hard. Do **not** brute-force register discovery unattended. Every
> step below that reads or writes the audio block must be done with serial
> console attached and a power cycle on hand.

## Scope

EA1 has **no ADAU1451 and no I2C codec** — that is EA3's `audio-dsp` path. EA1
audio = **HDMI audio** emitted by the SoC's integrated HDMI transmitter. The
userspace half (`audio` feature: ALSA + librespot + shairport) already exists and
needs nothing but a working card. What is missing is the **kernel ASoC backend**
that moves PCM samples out over HDMI.

## What the vendor path actually is (and why it is not in the GPL drop)

The stock Control4 HDMI-audio datapath is **proprietary Intel SMD/GDL**, not in
Control4's GPL kernel drop:

- **`ismdaudio` (Intel SMD audio driver).** The kernel-side SMD audio module. Its
  `pci_probe` is a ~3-byte stub — it does not map audio registers from the PCI
  function; the MMIO base comes from an OSAL mapping call (device name `"AUD_IO"`,
  a 3 MB window). The I²S render path was recovered from its DWARF debug info and
  lives in `ce5300-i2s.{c,h}` / `site/.../audio-regmap.md`, **but that is the
  analog/I²S render path, not HDMI.**
- **GDL `pd_hdmi` / `gdl_server` (userspace).** The HDMI "port driver" and display
  server that configure the HDMI transmitter — including the audio InfoFrame,
  channel-status bits, and the ACR N/CTS clock-recovery values — live in Intel's
  closed GDL userspace. **Not published.**
- **`c4-smd-pcm-audio`.** Control4's out-of-tree ASoC PCM/DMA platform shim. The
  GPL drop *references* it but **does not contain the source** — same gap that
  forced the DWARF reconstruction for the I²S path.

So there is no source to copy for the HDMI render path. It must be reconstructed.

## PCI functions and the clock-gate wall

From `site/src/content/docs/ea/audio-regmap.md` (recovered on a live EA3):

| BDF | ID | BAR0 | Size | Note |
|---|---|---|---|---|
| `01:06.0` | `8086:2e5f` | `0xdfd00000` | 512 KB | audio (claimed by `ismdaudio`) |
| `01:06.1` | `8086:2e5f` | `0xdfd80000` | 512 KB | audio |
| `01:06.2` | `8086:2e60` | `0xdfa80000` | 64 KB | audio (I²S render — what `ce5300-i2s` binds) |

The HDMI-audio FIFO is one of the SMD audio functions above; **which function /
which sub-block within it carries the HDMI stream is not yet established** (the
stock stack reaches it through the SMD port abstraction, not a fixed BAR offset
we have mapped).

**The hard wall:** the entire audio block sits behind the **Clock-and-Reset
Controller — CRC, PCI `00:00.2`, `8086:2e52`** — which **CEFDK never ungates**
(Control4's media stack did). Until the CRC ungates the audio block, *every*
audio register read hangs the SoC. **Ungating the CRC is a prerequisite for any
HDMI-audio work and is itself unsolved, even for EA3.** This is the first thing
the supervised session must crack.

## What GPL source *does* give us (HDMI side)

The IOSF sideband driver in the vendor GPL tree
(`drivers/char/iosf/`, `_ce5300_iosf.c` + `iosf_common.h`) exposes message-bus
**ports** relevant to HDMI audio clocking/PHY — these are reachable via the IOSF
mailbox, a different and safer mechanism than banging the audio MMIO directly:

| Port | ID | Relevance |
|---|---|---|
| `IOSF_PORT_HDMI_TX_AFE` | `0x82` | HDMI transmitter analog front-end (PHY) |
| `IOSF_PORT_HDMI_RX_AFE` | `0x83` | HDMI receiver AFE (not needed for output) |
| `IOSF_PORT_ADAC` | `0x81` | audio DAC island |
| `IOSF_PORT_APLL` | `0x8B` | audio PLL — the MCLK/TMDS-derived audio clock |
| `IOSF_PORT_PUNIT` | `0x04` | power/clock unit (candidate for the ungate) |

`audio_pvt_ce53xx_hal_set_pll_configuration_mode` + `apll_cr[8]` (from the I²S
DWARF) drive the audio PLL via `IOSF_PORT_APLL`; the HDMI audio clock (ACR) must
agree with the TMDS clock the transmitter is running. The IOSF driver gives a
non-MMIO way to inspect APLL/PUNIT/CRC state, which is the **safe first probe**.

## Register-discovery TODO (do in order, supervised)

1. **Ungate the audio block.** Via IOSF `PUNIT`/CRC (`8086:2e52`), determine and
   clear the clock-gate / reset holding the audio island. Confirm success by
   reading **one** known-answering register (BAR0 `+0x00` on `8086:2e60` already
   answers — see `ce5300-i2s.c` `ce5300_dump()`), then cautiously widening. Until
   a read of a TX/DMA register returns instead of hanging, stop — nothing else is
   safe.
2. **Locate the HDMI-audio FIFO/DMA.** With the block ungated, map the SMD audio
   function and find the HDMI stream's FIFO write port and DMA engine. The I²S
   engine in `ce5300-i2s.h` (linked-list DMA, 0x1000 DMA base / 0x2000 TX base,
   bit 28 = halt-after-descriptor) is the closest known relative — confirm
   whether HDMI reuses it or has its own.
3. **ACR N/CTS.** Find and compute the N and CTS registers for each sample rate
   vs the HDMI TMDS clock (HDMI 1.4 spec tables for N; CTS measured or fixed).
4. **Audio InfoFrame + channel status.** Find the InfoFrame RAM / FIFO and the
   IEC60958 channel-status bits; assemble for 2ch LPCM.
5. **Wire into the skeleton.** Fill `ce5300-hdmi.c`'s `prepare`/`trigger`/
   `pointer` stubs with the real ops. Keep the probe-only / `enable_pcm`-style
   gating pattern from `ce5300-i2s.c` so a half-finished backend cannot be armed
   by accident.
6. **(Optional) move off dummy to hdmi-codec.** Once EDID/ELD is wanted, register
   an `hdmi-audio-codec` platform device (enable `CONFIG_SND_SOC_HDMI_CODEC=m` in
   `features/audio-hdmi/linux.fragment`) and swap `COMP_DUMMY()` in
   `ce5300-ea1.c` for the shim.

## Supervised test plan

Prerequisites: serial console attached (EA1 console = optiplex COM3 @115200 per
memory), an HDMI sink connected, power cycle on hand.

1. Boot EA1 with the `audio-hdmi` feature. `S45ea-audio` modprobes
   `ce5300-hdmi → ce5300-ea1-board → ce5300-ea1`. Confirm `aplay -l` lists the
   `openhc-ea1` card and an HDMI playback device (`hw:0,0`). With the skeleton
   this enumerates but produces silence — that is expected.
2. **Only after step 1 of the TODO (block ungated, one register proven to
   answer):** cautiously dump candidate registers, one at a time, watching the
   serial console. A hang = that register is still gated → power-cycle, revisit
   the ungate.
3. As the backend fills in, validate with:
   ```
   speaker-test -D hw:0,0 -c 2 -t wav -l 2
   aplay -D hw:0,0 /usr/share/sounds/<48k-stereo>.wav
   ```
   over HDMI, checking the sink locks audio (correct rate, no dropouts) and that
   `.pointer` advances (no drift) before trusting librespot/shairport on it.
4. Only once `speaker-test` is clean end-to-end should the PCM be enabled by
   default (drop any `enable_pcm`-style gate) and `audio-hdmi` be considered
   functional rather than scaffolding.

## Files

```
sound/soc/ce5300/ce5300-hdmi.c         platform + cpu-DAI skeleton (no-op, no MMIO)
sound/soc/ce5300/ce5300-ea1.c          machine driver (card; cpu DAI -> snd-soc-dummy)
sound/soc/ce5300/ce5300-ea1-board.c    glue: registers the two platform devices
board/ea/common/features/audio-hdmi/linux.fragment   kernel config (modules)
board/ea/common/rootfs-overlay/etc/init.d/S45ea-audio shared loader (EA3 + EA1)
```

Enable with `audio-hdmi` in a board's `ohc.features` (set on ea1-v1, ea1-v2).
