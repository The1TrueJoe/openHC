# Stock LM3S1162 IO-MCU firmware — the way back

These are Control4's own images, byte-for-byte as pulled from a live HC-800's
`/control4/firmware/io/` (OS 3.x, unit `Main-HC800-000FFF57B978`, 2026-09-07).

| File | Size | sha256 |
|---|---|---|
| `700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin` | 65,794 | `83bd6b72…9094cec` |
| `IRBootloaderSerialLM3S1162.bin` | 4,068 | `1f3bf874…155e58c5` |
| `flash.config.xml` | 1,286 | the vendor's `.flash.config`, verbatim |

**They are here to be a restore path, not a reference to copy from.** This is the
MCU's equivalent of the sda2 factory partition: if our own firmware turns out to
be wrong, these put the unit back exactly as it shipped. The blanket `*.bin` in
`.gitignore` has an explicit exception for this directory for that reason —
losing them would make a bad flash unrecoverable without SWD.

The running MCU confirms this is the image it is executing: asked
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

These are Control4/Snap One proprietary binaries, redistributed here only as the
recovery artefact for hardware their owner already possesses. They are not part
of openHC, carry no openHC licence, and nothing in `../src` is derived from
their code — see the clean-room note in `../README.md`.
