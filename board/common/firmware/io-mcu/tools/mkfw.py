#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""openHC IO-MCU firmware images: the 64-byte header the driver reads, then the
bytes it writes to flash. Layout in ohc-iomcu-proto.h (OHC_IMG_*).

  mkfw.py image --part P --version V [--sram-top A] app.raw out.bin
  mkfw.py stock --part P stock.bin out.bin   wrap Control4's image (restore tests)
  mkfw.py info  image.bin
  mkfw.py hash  files...                     8 hex digits over the sources
"""
import argparse
import hashlib
import os
import struct
import sys
import zlib

MAGIC = b"OHCIOMCU"
HDR_LEN = 64
APP_BASE = 0x1000
FLASH_SIZE = 0x10000
VERSION_LEN = 32
PARTS = {"tm4c1231d5": 1, "lm3s1162": 2}
PART_NAMES = {v: k for k, v in PARTS.items()}
# vector table entries: 16 system + IRQs
VECTORS = {"tm4c1231d5": 16 + 107, "lm3s1162": 16 + 48}
C4_HDR = 0x100          # Control4's LM3S container: text header, 64 KB, CRC-16/ARC


def crc16_arc(data):
    crc = 0
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def header(part, load, payload, version):
    v = version.encode()
    if len(v) >= VERSION_LEN:
        raise SystemExit(f"version {version!r} is longer than {VERSION_LEN - 1} bytes")
    h = MAGIC + struct.pack("<HHIIII", 1, HDR_LEN, PARTS[part], load, len(payload),
                            zlib.crc32(payload))
    h += v.ljust(VERSION_LEN, b"\0")
    return h + struct.pack("<I", zlib.crc32(h))


def parse(img):
    """(fields, payload) of an image, or raises ValueError."""
    if len(img) < HDR_LEN or img[:8] != MAGIC:
        raise ValueError("not an openHC IO-MCU image")
    ver, hlen, part, load, size, crc = struct.unpack_from("<HHIIII", img, 8)
    (hcrc,) = struct.unpack_from("<I", img, 60)
    if ver != 1 or hlen != HDR_LEN or zlib.crc32(img[:60]) != hcrc:
        raise ValueError("bad header")
    payload = img[HDR_LEN:]
    if len(payload) != size or zlib.crc32(payload) != crc:
        raise ValueError("payload size or CRC does not match the header")
    if load < APP_BASE or load + size > FLASH_SIZE or size % 4:
        raise ValueError(f"{size} bytes at {load:#x} leave the app slot")
    version = img[28:60].split(b"\0", 1)[0].decode()
    return dict(part=part, load=load, size=size, crc=crc, version=version), payload


def check_vectors(raw, part, sram_top):
    n = VECTORS[part]
    if len(raw) < n * 4:
        raise SystemExit("image shorter than its vector table")
    sp, pc = struct.unpack_from("<II", raw, 0)
    bad = [] if 0x20000000 < sp <= sram_top else ["SP"]
    for i in range(1, n):
        (v,) = struct.unpack_from("<I", raw, i * 4)
        if v and (not v & 1 or not APP_BASE <= (v & ~1) < FLASH_SIZE):
            bad.append(i)
    if bad:
        raise SystemExit(f"bad vector table ({bad[:6]}): refusing to make a flashable image")
    print(f"  vectors: SP={sp:#010x} reset={pc:#010x}, {n} entries valid")


def cmd_image(a):
    raw = open(a.raw, "rb").read()
    check_vectors(raw, a.part, int(a.sram_top, 0))
    raw += b"\xff" * (-len(raw) % 4)
    if APP_BASE + len(raw) > FLASH_SIZE:
        raise SystemExit(f"{len(raw)} bytes do not fit the {FLASH_SIZE - APP_BASE}-byte app slot")
    open(a.out, "wb").write(header(a.part, APP_BASE, raw, a.version) + raw)
    print(f"  {a.out}: {len(raw)} bytes at {APP_BASE:#x}, {a.version}")


def stock_payload(data, part):
    """The app-slot bytes of a Control4 image, and its version string."""
    if part == "lm3s1162":
        if len(data) != C4_HDR + FLASH_SIZE + 2:
            raise SystemExit("not a Control4 LM3S container (256 + 65536 + 2 bytes)")
        flash = data[C4_HDR:C4_HDR + FLASH_SIZE]
        (crc,) = struct.unpack_from("<H", data, C4_HDR + FLASH_SIZE)
        if crc16_arc(flash) != crc:
            raise SystemExit("Control4 container CRC-16 does not match")
        hdr = data[:C4_HDR]
        i = hdr.find(b"FWVERS:")
        version = hdr[i + 8:].split(b"\0", 1)[0].decode() if i >= 0 else "stock"
    else:
        if len(data) != FLASH_SIZE:
            raise SystemExit("not a raw 64 KB TM4C flash image")
        flash = data
        version = "stock"
    # the whole slot, so nothing of openHC's is left past the end of stock's code
    return flash[APP_BASE:], version


def cmd_stock(a):
    app, version = stock_payload(open(a.stock, "rb").read(), a.part)
    open(a.out, "wb").write(header(a.part, APP_BASE, app, version) + app)
    print(f"  {a.out}: Control4 {version}, {len(app)} bytes at {APP_BASE:#x}")


def cmd_info(a):
    f, _ = parse(open(a.image, "rb").read())
    print(f"part {PART_NAMES.get(f['part'], f['part'])}  load {f['load']:#x}  "
          f"size {f['size']}  crc {f['crc']:#010x}  version {f['version']}")


def cmd_hash(a):
    h = hashlib.sha256()
    # by name and content, not path: the build directory must not change it
    for p in sorted(a.files, key=os.path.basename):
        h.update(os.path.basename(p).encode() + b"\0" + open(p, "rb").read())
    print(h.hexdigest()[:8])


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("image")
    p.add_argument("--part", choices=PARTS, required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--sram-top", default="0x20004000")
    p.add_argument("raw")
    p.add_argument("out")
    p.set_defaults(fn=cmd_image)
    p = sub.add_parser("stock")
    p.add_argument("--part", choices=PARTS, required=True)
    p.add_argument("stock")
    p.add_argument("out")
    p.set_defaults(fn=cmd_stock)
    p = sub.add_parser("info")
    p.add_argument("image")
    p.set_defaults(fn=cmd_info)
    p = sub.add_parser("hash")
    p.add_argument("files", nargs="+")
    p.set_defaults(fn=cmd_hash)
    a = ap.parse_args(argv)
    try:
        a.fn(a)
    except ValueError as e:
        raise SystemExit(str(e))


if __name__ == "__main__":
    sys.exit(main())
