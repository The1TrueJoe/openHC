#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""mkfw.py: images round-trip, a bad table or CRC is refused, stock wraps."""
import os
import struct
import subprocess
import sys
import tempfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mkfw  # noqa: E402

failed = 0


def check(cond, what):
    global failed
    if not cond:
        failed += 1
        print("FAIL:", what)


def vectors(n, sp=0x20004000, pc=0x1101):
    v = [sp, pc] + [0x1201] * (n - 2)
    return struct.pack(f"<{n}I", *v)


with tempfile.TemporaryDirectory() as d:
    raw = os.path.join(d, "app.raw")
    out = os.path.join(d, "app.bin")
    open(raw, "wb").write(vectors(64) + b"\x01\x02\x03")
    mkfw.main(["image", "--part", "lm3s1162", "--version", "openhc-1.0.0+abc", raw, out])
    f, payload = mkfw.parse(open(out, "rb").read())
    check(f["part"] == 2 and f["load"] == 0x1000 and f["version"] == "openhc-1.0.0+abc", f)
    check(len(payload) % 4 == 0 and payload[:256] == vectors(64), "payload")

    img = bytearray(open(out, "rb").read())
    img[100] ^= 1
    try:
        mkfw.parse(bytes(img))
        check(False, "payload CRC not checked")
    except ValueError:
        pass
    img = bytearray(open(out, "rb").read())
    img[30] ^= 1
    try:
        mkfw.parse(bytes(img))
        check(False, "header CRC not checked")
    except ValueError:
        pass

    # an even reset vector (ARM, not Thumb) is a dead part: refused
    open(raw, "wb").write(vectors(64, pc=0x1100))
    r = subprocess.run([sys.executable, os.path.join(HERE, "mkfw.py"), "image", "--part",
                        "lm3s1162", "--version", "x", raw, out], capture_output=True)
    check(r.returncode != 0, "bad vector table accepted")

    # Control4's LM3S container: CRC-16/ARC over 64 KB, header text before it
    flash = bytearray(b"\xff" * 0x10000)
    flash[0x1000:0x1000 + 256] = vectors(64)
    hdr = (b"\x00\x0cIR_Processor\xff\xffFWVERS:\x0003.26.15\x00").ljust(256, b"\xff")
    stock = hdr + bytes(flash) + struct.pack("<H", mkfw.crc16_arc(flash))
    sp = os.path.join(d, "stock.bin")
    open(sp, "wb").write(stock)
    mkfw.main(["stock", "--part", "lm3s1162", sp, out])
    f, payload = mkfw.parse(open(out, "rb").read())
    check(f["version"] == "03.26.15", f["version"])
    check(len(payload) == 0xf000, "stock writes the whole slot")
    check(mkfw.crc16_arc(b"123456789") == 0xBB3D, "CRC-16/ARC check value")

    open(sp, "wb").write(stock[:-1] + bytes([stock[-1] ^ 1]))
    r = subprocess.run([sys.executable, os.path.join(HERE, "mkfw.py"), "stock", "--part",
                        "lm3s1162", sp, out], capture_output=True)
    check(r.returncode != 0, "bad stock CRC accepted")

    # the header layout the driver reads
    img = open(out, "rb").read()
    check(img[:8] == b"OHCIOMCU" and struct.unpack_from("<I", img, 60)[0] == zlib.crc32(img[:60]),
          "header layout")

print(f"mkfw tests: {'ok' if not failed else f'{failed} failed'}")
sys.exit(1 if failed else 0)
