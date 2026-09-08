# Stock LM3S1162 IO-MCU firmware — the way back

**The images are not in this repository, and must not be.** They are
Control4/Snap One proprietary firmware. That the owner of an HC-800 already has
a copy on their own unit is a reason they may keep one; it is not a licence for
this project to hand one to everybody who clones the repo. `.gitignore` has no
exception for them.

Get your own, off your own unit, before flashing anything:

```sh
scp root@<your-hc800>:/control4/firmware/io/*.bin \
    board/hc800/firmware/io-mcu/lm3s1162/vendor/
```

They land here, git ignores them, and every tool in this directory finds them.

## What you should have

Checksums, so you can confirm you pulled the right images. These are facts about
the files, not the files — recording them is fine, shipping them is not.

| File | Size | sha256 |
|---|---|---|
| `700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin` | 65,794 | `83bd6b72…9094cec` |
| `IRBootloaderSerialLM3S1162.bin` | 4,068 | `1f3bf874…155e58c5` |
| `flash.config.xml` | 1,286 | the vendor's `.flash.config`, verbatim |

Observed on OS 3.x, unit `Main-HC800-000FFF57B978`, 2026-09-07. `flash.config.xml`
is kept here: it is a short XML manifest, not a program, and it is what documents
the board-to-image mapping described below.

**They are a restore path, not a reference to copy from.** This is the MCU's
equivalent of the sda2 factory partition: if our own firmware turns out to be
wrong, these put the unit back exactly as it shipped. Without them a bad flash is
unrecoverable without SWD — which is why you should fetch them BEFORE you flash,
not after.

The running MCU confirms which image it is executing: asked
`FIRMWARE_VERSION_GET` on `/dev/ttyS3`, it answers `03.26.15`, matching both the
filename and the `FWVERS:` field inside the header.

## What `flash.config.xml` says

`hc800` and `hc250` map to **the same pair**. So does the naming inside the app
image, which advertises `Processor: LM3S1162 / LM3S615 / LM3S811 / LM3S815` —
"MultiConfig" means multi-**processor**, not multi-board. That matters when
reading the image: features present in it are not necessarily features of *this*
board. The clearest example is the third UART, which belongs to the HC-250 half
of the set; on the HC-800 the rear RS-232 ports are host 16550As.

## Container format

Decoded, and `../tools/mkimage.py` reproduces this image's CRC exactly:

```
[0x000 .. 0x0FF]      256-byte text header, 0xFF padded
[0x100 .. 0x100FF]    64 KB raw flash image (flash 0x0000..0xFFFF)
                      low 4 KB left 0xFF — the bootloader's region
[0x10100 .. 0x10101]  CRC-16/ARC over the whole 64 KB, little-endian
```

Check any image, ours or theirs:

```sh
python3 ../tools/mkimage.py --verify <image>
```

## Licence

Control4/Snap One proprietary. Not part of openHC, carrying no openHC licence,
and not redistributed by this project. Nothing in `../src` is derived from their
code — see the clean-room note in `../README.md`.

An earlier revision of this file argued they could be committed because they are
"the recovery artefact for hardware their owner already possesses". That was
wrong: a public repository redistributes to everyone, not to owners, and no
amount of purpose makes that a licence.
