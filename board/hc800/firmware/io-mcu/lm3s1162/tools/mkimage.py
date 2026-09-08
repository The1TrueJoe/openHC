#!/usr/bin/env python3
"""Wrap a raw application .bin into Control4's LM3S1162 firmware container.

The container was decoded from the stock image
(700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin,
65,794 bytes) and is:

    [0x000 .. 0x0FF]      256-byte text header, 0xFF padded
    [0x100 .. 0x100FF]    64 KB raw flash image, flash 0x0000..0xFFFF
                          (the low 4 KB is the bootloader's and stays 0xFF)
    [0x10100 .. 0x10101]  CRC-16/ARC of the whole 64 KB, little-endian

The CRC is the part that had to be found rather than read: poly 0x8005,
init 0x0000, reflected in and out, no final xor, computed over all 65,536 flash
bytes. Verified against the stock image, which carries 0xA578.

The header is a run of NUL-terminated strings separated by 0xFF, opening with a
one-byte-length-prefixed product name:

    00 0C "IR_Processor"  FF FF
    "Copyright ..." 00 FF FF
    "FWVERS:" 00 "03.26.15" 00 FF FF
    "DATE: ..." 00

Usage:
    mkimage.py build/hc800/io-mcu.bin build/hc800/io-mcu-fw.bin \\
        [--version 0.1.0] [--name IR_Processor]
    mkimage.py --verify <stock-or-built-image>
"""
import argparse
import datetime
import sys

FLASH_SIZE = 0x10000
HEADER_SIZE = 0x100
APP_BASE = 0x1000          # the bootloader owns flash 0x0000..0x0FFF


def crc16_arc(data: bytes) -> int:
    """CRC-16/ARC: poly 0x8005 reflected (0xA001), init 0, no final xor."""
    crc = 0
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def build_header(name: str, version: str) -> bytes:
    stamp = datetime.datetime.now().strftime("DATE: %-m/%-d/%Y %-I:%M:%S %p")
    h = bytearray()
    # NUL after the name, then the 0xFF separator. Every field in the stock
    # header is NUL-terminated and an earlier version of this dropped it here
    # only — which produced a name running straight into the 0xFF padding. If
    # the bootloader reads this as a C string that is a buffer overrun waiting
    # to happen, and it costs one byte to match the vendor exactly:
    #   stock: 00 0c "IR_Processor" 00 ff ff
    h += bytes([0x00, len(name)]) + name.encode() + b"\x00\xff\xff"
    h += b"Copyright openHC\x00\xff\xff"
    h += b"FWVERS:\x00" + version.encode() + b"\x00\xff\xff"
    h += stamp.encode() + b"\x00"
    if len(h) > HEADER_SIZE:
        sys.exit(f"header is {len(h)} bytes, must fit in {HEADER_SIZE}")
    return bytes(h) + b"\xff" * (HEADER_SIZE - len(h))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("infile", nargs="?")
    ap.add_argument("outfile", nargs="?")
    ap.add_argument("--name", default="IR_Processor")
    ap.add_argument("--version", default="0.1.0")
    ap.add_argument("--verify", metavar="IMAGE")
    a = ap.parse_args()

    if a.verify:
        raw = open(a.verify, "rb").read()
        want = HEADER_SIZE + FLASH_SIZE + 2
        if len(raw) != want:
            return int(bool(print(f"FAIL size {len(raw)}, expected {want}")))
        flash = raw[HEADER_SIZE:HEADER_SIZE + FLASH_SIZE]
        got = int.from_bytes(raw[-2:], "little")
        calc = crc16_arc(flash)
        print(f"header : {raw[:64]!r}")
        print(f"crc    : stored 0x{got:04x}  computed 0x{calc:04x}  "
              f"{'OK' if got == calc else 'MISMATCH'}")
        return 0 if got == calc else 1

    if not a.infile or not a.outfile:
        ap.error("need infile and outfile (or --verify)")

    app = open(a.infile, "rb").read()
    if len(app) > FLASH_SIZE - APP_BASE:
        return int(bool(print(
            f"application is {len(app)} bytes, slot is {FLASH_SIZE - APP_BASE}")))

    # 0xFF is erased flash. Leaving the low 4 KB erased is what keeps Control4's
    # serial bootloader intact — it is the only recovery path that does not need
    # SWD, so this padding is load bearing, not cosmetic.
    flash = b"\xff" * APP_BASE + app + b"\xff" * (FLASH_SIZE - APP_BASE - len(app))
    assert len(flash) == FLASH_SIZE

    out = build_header(a.name, a.version) + flash
    out += crc16_arc(flash).to_bytes(2, "little")
    open(a.outfile, "wb").write(out)
    print(f"{a.outfile}: {len(out)} bytes "
          f"(app {len(app)} at 0x{APP_BASE:04x}, crc 0x{crc16_arc(flash):04x})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
