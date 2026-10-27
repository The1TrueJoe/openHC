# ADAU1451 SigmaDSP (EA3)

The EA3 routes its analog audio through an Analog Devices **ADAU1451**
SigmaDSP: CE5300 I²S1 → ADAU1451 → AK4621 → the analog jacks. It sits on
**i2c-3 at 0x38**, with reset on `dsp_reset` (gpio101) and the DAC soft mute on
`dac_mute` (gpio61).

The part has **no program of its own**. It comes out of reset with its PLL off
and its core idle (read on hardware under openHC, 2026-10-26: PLL registers 0,
core status 0, RAM random), and the host loads everything over I²C; on a stock
unit a userspace loader does it at boot. openHC loads it from the codec driver,
out of one firmware file:

1. **the register init** in `ea3.json` — clocks, serial ports, routing, decoded
   from a stock unit's registers ([below](#the-register-init));
2. **the program** — openHC's own, built by SigmaStudio+ in CI and handed to the
   image build. It is never committed;
3. **the parameters** — the filter tables behind every tone and EQ knob, and the
   knob cells.

Until there is a program the build makes a file with the parameters only,
byte for byte what it made before, and the driver writes nothing (see
[No program yet](#no-program-yet)).

## How openHC drives it — all stock Linux

| Piece | What it is |
|---|---|
| `ea3.json` | **The configuration, in code.** Every knob: the cell it lives in, the table behind it, and the filter design that fills the table. |
| `program.json` | **The program's design, in code**: SigmaStudio+ scripting calls, run in order by CI (`dsp-program.yml`) — see [Building the program](#building-the-program). |
| `export2program.py` | Turns SigmaStudio+'s export into `mkfw.py`'s `--program` input, and its parameter map into `params.json`. Standard library Python. |
| `mkfw.py` | Builds `ea3.json`, and the program if given (`--program`), into a firmware file in the kernel's own SigmaDSP format (`ADISIGM` v2, mainline chunk tags). Standard library Python. |
| `adau1451-fw.mk`, `Config.in` | The build rule: runs `mkfw.py` at image build and installs the file and the profile. Selected by the `audio-dsp` feature (`BR2_PACKAGE_ADAU1451_FW`); `BR2_PACKAGE_ADAU1451_FW_PROGRAM` is the path of the program, empty for none. |
| `/lib/firmware/openhc/adau1451-ea3.bin` | That file, on the box. |
| the board DTB | `firmware-name` names the file; `control4,program-version` is the program's version cell and the value it must hold, checked after the load. The card is a stock `simple-audio-card`. |
| `adau1451-c4` | The codec driver. It hands the file to the kernel's **sigmadsp** core (the one the mainline ADAU1701/ADAU17x1 drivers use), which writes it at bind and turns each named cell into an **ALSA control**. Around that the driver does the sequencing: soft reset, hibernate, the wait for PLL lock, start. |
| ohc-audiod | Reads the knob list from the installed copy of `ea3.json` and sets the controls with `amixer`, exactly where the HC-800 sets its software tone stage. The web UI shows those knobs. |

So the DSP is tuned with ordinary ALSA tools:

```
amixer -c openhcea contents                        # every DSP control
amixer -c openhcea cset name='DSP Bass' 0,0,0,30   # bass +3 dB (index 24 + 3/0.5)
```

Every control is 4 bytes, big-endian: an **index 0..48** into a 49-entry table,
**24 neutral**. Tone and EQ steps are `step_db` apart (0.5 dB → ±12 dB);
balance is 100/24 % a step. ohc-audiod does that arithmetic; the API and UI
speak dB and −100..100.

## Loading the part

The firmware's data chunks are written in file order, which `mkfw.py` makes the
order the part needs, and the driver wraps them in the datasheet's
initialization sequence (Table 24), which is also the stock loader's:

1. `SOFT_RESET` 0 → 1: every control register to its default, except the PLL's.
2. `HIBERNATE` 0 → 1, 255 ms.
3. The register init, one register at a time. It turns the PLL off, sets it,
   and turns it on; **while the PLL locks the control port answers nothing**,
   so the driver's write op polls `PLL_LOCK` (0xF004) after that write, for up
   to 50 ms (datasheet worst case 10.7 ms), and fails the load if it never
   locks — that means no MCLK.
4. The program memory (0xC000…), then DM0 (0x0000…) and DM1 (0x6000…) as the
   program came with them, then the parameters on top.
5. `START_CORE` 0 → 1, then `HIBERNATE` 0. Stock's loader lifts hibernate last,
   which is why a stock unit reads 0 there while it plays.
6. The version cell from `control4,program-version` must hold its value, or the
   core is hibernated again and the card does not bind.

The program file `mkfw.py --program` takes is plain text: `@<address>` starts a
block, hex bytes follow (commas, `0x`, comments all fine) — the address and
byte array of each memory write a SigmaStudio(+) export contains
(`Program_Data`, `Param_Data`, `DM1_DATA_Data`). Memory only: control registers
are `ea3.json`'s, and `mkfw.py` refuses them.

## Building the program

There is no open compiler for the ADAU145x (its instruction set is not public),
so the program is built by Analog Devices' SigmaStudio+, on a GitHub-hosted
Windows runner, with no GUI: `.github/workflows/dsp-program.yml` installs it
through [sigmastudio-plus-action](https://github.com/The1TrueJoe/sigmastudio-plus-action)
and runs `program.json` with the action's headless scripting host. The calls
are SigmaStudio+'s own scripting API (`AddShapeV2`, `AddConnectionV2`,
`UpdatePropertiesV2`, `LinkCompileConnect`, `Export`, …).

The design starts from ADI's *ADAU1452 Filtering Compressed Input Signal*
example, which ships with SigmaStudio+, because its PC → USBi → DSP link is the
one part of a system the scripting API cannot add (the compiler refuses a
processor with no host link; nothing is downloaded over it). It migrates the
example's processor to the ADAU1451, removes the example's algorithms, and
builds openHC's chain:

| Block (label) | What |
|---|---|
| `ASRCInput_0` | ASRC 0's two channels: serial input 3, the CE5300's I²S1, converted to the core's rate |
| `Volume` | a stereo volume (two gains in DM0) |
| `Output_*` | left and right to the first two channels of **all four** serial outputs (channels 0/1, 16/17, 32/33, 40/41), until it is known which one feeds the AK4621 |
| `Version` | a DC cell holding the program's version (1, as 8.24), for the driver's check |

The export becomes two files in the run's `adau1451-program` artifact:

* `program.txt`: the memory writes of the export's default download, which is
  `mkfw.py --program`'s format; and
* `params.json`: every parameter's address, by block label and parameter name
  (`Volume.Gain_Channel0` → DM0 20), so the profile can name a cell instead of
  hard-coding an address that moves whenever the design does.

Both are generated code under ADI's license: the artifact is kept three days,
and nothing exported is ever committed. **The image build does not use the
program yet**, and `ea3.json` still describes stock's program; the next stage
builds the knobs' blocks into the design and moves `ea3.json` to the labels.

### No program yet

A file without a program holds parameters only, which mean something only to
the program they were made for. Then the driver checks first: unless the DSP is
already running that program — after reset it never is — it writes nothing,
makes no controls, and the part stays as it came up.

## The register init

Read off a stock EA3 (2026-10-26) and decoded against the public ADAU1450/1451/
1452 datasheet (Rev. C). `ea3.json` lists each register with its meaning; the
init holds what differs from the default after a soft reset, plus the PLL.
Status registers are left out: `PLL_LOCK`, `CLK_GEN3_LOCK`, `CORE_STATUS`, the
panic flags, the program counters (0xF461 counts while it runs), and the
undocumented 0xF431 and 0xF800–0xF847.

* **Clock:** a 24.576 MHz MCLK, ÷8 into the PLL (3.072 MHz), ×96 = 294.912 MHz.
  CLKOUT off.
* **Every serial port is a slave.** The DSP drives no BCLK or LRCLK at all.
* **The core runs on serial input 0's frame clock** (`START_PULSE` = 8,
  LRCLK_IN0). Serial inputs 0–2 and all four outputs are at their defaults:
  slaves to BCLK_IN0/LRCLK_IN0 (inputs) and BCLK_OUT0/LRCLK_OUT0 (outputs),
  I²S, 24-bit, two 32-bit slots.
* **Serial input 3 is the odd one out:** slave to its own BCLK_IN3/LRCLK_IN3
  (`SERIAL_BYTE_3_0` = 0x6c00), and its channels 40/41 feed **ASRC 0**, which
  converts them to the outputs' rate (`ASRC_INPUT0` = 0xa1, `ASRC_OUT_RATE0` = 1).
  ASRCs 1–7 are muted. An ASRC is only needed for a source with a clock of its
  own, so **SDATA_IN3 is almost certainly the CE5300's I2S1**, with the CE5300
  (not the DSP) driving its clocks, whatever the vendor's ASoC link (codec
  master) said.
* **Every output channel comes from the DSP core** (`SOUT_SOURCE0..23` = 2);
  which output pin carries the program's output to the AK4621 is the program's
  business, not the registers'.
* The clock generators are set (÷8, ÷8, ÷5) but feed nothing: no port is a
  master and neither the core nor an ASRC selects them.
* **MP6 and MP7** are outputs with pull-ups, driven by the program.
* Output pads: slow slew, no pull-down; **SDATA_OUT3 alone is set to the fastest
  slew.**

Not settled by the registers, and left for hardware or the board:

* **Which SDATA_OUT feeds the AK4621.** SDATA_OUT3's own slew setting is a hint,
  not proof. Until it is known, a program can send its output to the first pair
  of all four outputs.
* **What clocks LRCLK_IN0/OUT0** — the core's frame clock. Likely the AK4621 or
  the CE5300; whatever it is must run before the core does anything.
* **What MP6/MP7 drive** on the board.
* **The latched panic on the stock unit**, `PANIC_CODE` 0x0008: a parity error
  in program memory 1 (`ERR_PM1`). Possibly from reading program memory back
  over I²C during the dump, possibly from unloaded program memory; it is not
  config and the init does not touch it.

## The knobs

| Knob | Cell | Table | Design (`ea3.json`) |
|---|---|---|---|
| Mode | 0x0db5 (8 words) | — | selector: **Tone** (boot) / EQ / Flat / Test tone |
| Loudness | 0x0db1 (4 words) | — | selector: Off (boot) / On |
| Bass | 0x0163 | 0x01dc | RBJ low shelf, 120 Hz, slope 1 — the HC-800's |
| Treble | 0x0dc1 | 0x03d6 | RBJ high shelf, 8 kHz, slope 1 — the HC-800's |
| Balance | 0x0dc2 | 0x0091 / 0x00c2 | linear, one side to silence — the HC-800's |
| EQ 31 Hz … 16 kHz | 0x0dc3 … 0x0dcc | 0x02d9, 0x04d3 … 0x0cbb | RBJ peaking, Q 1.414 |

Plus cells written once at boot: **Volume 0**, Max Volume 100, the
volume-curve select, Input Gain 24 (0 dB), the test oscillator's level, the
program's mute (released — stock's own "on" words), and a read-only
`ReadBack DSP Program` (the version cell).

**The mode decides which knobs act.** The program's output selector has four
paths, one per channel: Tone (bass and treble), EQ (the ten bands), Flat (stock's
"Bypass", what stock idles on), and Test tone (the program's oscillator). They
are alternatives, not a chain. The program boots its right-channel selectors and
its mute half set, which is why the firmware writes all three.

## Silent unless something plays

* **At boot** the DSP's Volume is written 0 and the DAC's soft mute is held.
  ohc-audiod restores the output's saved level and tone, *then* raises Volume to
  its `run` value (100 — the output's level lives in ALSA, as on the HC-800).
* **While idle** the DAC stays muted: the codec's ASoC `mute_stream` releases it
  when a stream is prepared and asserts it again when the last one closes, so
  there is no hiss on the jacks between songs.

To change a design, edit `ea3.json` and rebuild. The bass and treble shelves
match the HC-800's software stage, so a setting sounds the same on both boxes.

## Where the map came from

Read off a running stock EA3 (2026-10-26) over I²C, read-only:

* **Tables:** twelve 253-word blocks at `0x01d5 + k·0xfd` — a 7-word header,
  then 49 biquads stored `B2 B1 B0 −A2 −A1` in 8.24 fixed point. With stock's
  own designs, `mkfw.py` reproduces **all ten EQ tables bit for bit**, which is
  what proves the layout. Stock's 0 dB entry is the identity biquad; ours too.
* **Cells:** the program's named DC-input cells, from the control chunks of
  the unit's `ea3-1451.bin`; the live values match (all 24, Volume 0,
  SampleRate 48000, version 10).
* **Balance tables:** two 49-word gain tables at 0x0091 and 0x00c2, mirror
  images of each other.
* **Program memory reads back as zeros** over I²C, so a running program cannot
  be read back.

No Control4 or Analog Devices file is in this tree, and none is needed: the
addresses are facts about the unit, and every table is computed here.
`ea3-1451.bin` and Analog Devices' `ea3-dsp-params.h` are never committed.

**Cell → table pairing: confirmed on a stock unit.** Moving each of stock's own
controls one at a time (its driver wrote the cell) moved exactly that cell and
the header of exactly the block `ea3.json` pairs it with. The same diffs found
the mode and loudness selectors (written through the safeload registers at
0x18–0x1e) and the mute pair; stock's master volume is the Volume cell, which
the program turns into a gain through its log table at 0x0164.

## Still to confirm on hardware

* **The load** — none of it has run on a unit yet: the soft reset, the PLL lock
  wait (and that MCLK reaches the DSP under openHC at all), the program, the
  start.
* **Which balance table is the left jack** (assumed 0x0091).
* **What each mode sounds like** — the names are stock's; whether Tone also
  carries the EQ is for the ears.
* **Listening.** Nothing plays until the CE5300 I²S PCM is finished
  (`ce5300_i2s.enable_pcm`); until then the card has the DSP controls and a
  PCM that refuses to open.

## Not used on the EA1, and the AK4621

The EA1 has no DSP. The AK4621 between the DSP and the jacks has no control
port in use — stock only releases `codec_reset` — so it is strapped into
hardware mode and needs no driver.
