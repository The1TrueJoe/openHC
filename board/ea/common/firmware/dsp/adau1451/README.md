# ADAU1451 SigmaDSP (EA3)

The EA3 routes its analog audio through an Analog Devices **ADAU1451**
SigmaDSP: CE5300 I²S1 → ADAU1451 → AK4621 → the analog jacks. It sits on
**i2c-3 at 0x38**, with reset on `dsp_reset` (gpio101) and the DAC soft mute on
`dac_mute` (gpio61).

The part **boots its program by itself from the board's EEPROM** — Control4's
"SamX rev 10" program, which reports `10` in its version cell. openHC never
loads program memory. What openHC owns is the **parameter RAM** that program
reads: the filter tables behind every tone and EQ knob, and the knob cells.

## How openHC drives it — all stock Linux

| Piece | What it is |
|---|---|
| `ea3.json` | **The configuration, in code.** Every knob: the cell it lives in, the table behind it, and the filter design that fills the table. |
| `mkfw.py` | Builds `ea3.json` into a firmware file in the kernel's own SigmaDSP format (`ADISIGM` v2). Standard library Python, run at build time by the `adau1451-fw` package. |
| `/lib/firmware/openhc/adau1451-ea3.bin` | That file, on the box. |
| the board DTB | `firmware-name` names the file; `control4,program-version = <0x0dd0 10>` is the check that the running program is the one the file describes. The card is a stock `simple-audio-card`. |
| `adau1451-c4` | The codec driver. It hands the file to the kernel's **sigmadsp** core (the one the mainline ADAU1701/ADAU17x1 drivers use), which writes the tables at bind and turns each named cell into an **ALSA control**. |
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

## The knobs

| Knob | Cell | Table | Design (`ea3.json`) |
|---|---|---|---|
| Bass | 0x0163 | 0x01dc | RBJ low shelf, 120 Hz, slope 1 — the HC-800's |
| Treble | 0x0dc1 | 0x03d6 | RBJ high shelf, 8 kHz, slope 1 — the HC-800's |
| Balance | 0x0dc2 | 0x0091 / 0x00c2 | linear, one side to silence — the HC-800's |
| EQ 31 Hz … 16 kHz | 0x0dc3 … 0x0dcc | 0x02d9, 0x04d3 … 0x0cbb | RBJ peaking, Q 1.414 |

Plus cells written once at boot: **Volume 0**, Max Volume 100, the
volume-curve select, Input Gain 24 (0 dB), the test oscillator's level, and a
read-only `ReadBack DSP Program` (the version cell).

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
* **Program memory reads back as zeros**, so the program itself cannot be read
  over I²C — it only exists in the EEPROM.

No Control4 or Analog Devices file is in this tree, and none is needed: the
addresses are facts about the unit, and every table is computed here.
`ea3-1451.bin` and Analog Devices' `ea3-dsp-params.h` are never committed.

## Still to confirm on hardware

* **Cell → table pairing.** Every cell read 24 on the dump, so which cell
  drives which block was taken from the cells' names, which match the blocks'
  frequencies. To check one: set a cell (`amixer … cset name='DSP EQ 1 kHz'
  0,0,0,30`) and read words `+5` and `+6` of each block header — the block
  whose index follows is the one that cell drives.
* **Which balance table is the left jack** (assumed 0x0091).
* **Listening.** Nothing plays until the CE5300 I²S PCM is finished
  (`ce5300_i2s.enable_pcm`); until then the card has the DSP controls and a
  PCM that refuses to open.

## Not used on the EA1, and the AK4621

The EA1 has no DSP. The AK4621 between the DSP and the jacks has no control
port in use — stock only releases `codec_reset` — so it is strapped into
hardware mode and needs no driver.
