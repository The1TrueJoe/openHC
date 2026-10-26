#!/usr/bin/env python3
"""Build openHC's SigmaDSP firmware file for an ADAU1451 from a profile.

    mkfw.py ea3.json adau1451-ea3.bin

The output is the kernel's own SigmaDSP firmware format (sound/soc/codecs/
sigmadsp.c, "ADISIGM" version 2), so the stock sigmadsp core loads it and turns
it into ALSA controls; there is no openHC loader. It holds three kinds of chunk:

  data      parameter RAM to write once at boot: every knob's 49-entry table,
            computed here from the profile's designs, and the cells' values
  control   one ALSA control per knob and cell, 4 bytes at the cell's address
  rates     the one sample rate the program runs at

No program memory: the DSP boots its program from the board's EEPROM, and this
file only fills in the parameters that program reads.

Standard library only; the build host's python3 runs it.
"""
import json
import math
import struct
import sys
import zlib

# The program's tables are 49 entries, the middle one neutral; only the step
# between entries is the profile's to choose.
STEPS = 49
FLAT = 24

TAG_DATA, TAG_CONTROL, TAG_RATES = 0, 1, 2


def addr(s):
    return int(s, 0)


def fixed(x):
    """A parameter word: signed 8.24 fixed point, big-endian."""
    v = round(x * (1 << 24))
    if not -(1 << 31) <= v < (1 << 31):
        raise ValueError(f"{x} does not fit 8.24")
    return struct.pack(">i", v)


def biquad(f, db, fs):
    """RBJ audio-EQ-cookbook coefficients, normalised: (b0, b1, b2, a1, a2)."""
    kind = f["type"]
    A = 10 ** (db / 40)
    w0 = 2 * math.pi * f["freq"] / fs
    c, s = math.cos(w0), math.sin(w0)
    if kind == "peak":
        alpha = s / (2 * f["q"])
        b = (1 + alpha * A, -2 * c, 1 - alpha * A)
        a = (1 + alpha / A, -2 * c, 1 - alpha / A)
    elif kind in ("lowshelf", "highshelf"):
        alpha = s / 2 * math.sqrt((A + 1 / A) * (1 / f["slope"] - 1) + 2)
        sa = 2 * math.sqrt(A) * alpha
        if kind == "lowshelf":
            b = (A * ((A + 1) - (A - 1) * c + sa), 2 * A * ((A - 1) - (A + 1) * c),
                 A * ((A + 1) - (A - 1) * c - sa))
            a = ((A + 1) + (A - 1) * c + sa, -2 * ((A - 1) + (A + 1) * c),
                 (A + 1) + (A - 1) * c - sa)
        else:
            b = (A * ((A + 1) + (A - 1) * c + sa), -2 * A * ((A - 1) + (A + 1) * c),
                 A * ((A + 1) + (A - 1) * c - sa))
            a = ((A + 1) - (A - 1) * c + sa, 2 * ((A - 1) - (A + 1) * c),
                 (A + 1) - (A - 1) * c - sa)
    else:
        raise ValueError(f"unknown filter type {kind!r}")
    return tuple(x / a[0] for x in b) + (a[1] / a[0], a[2] / a[0])


def filter_table(f, fs, step_db):
    """49 biquads, step_db apart around 0 dB. The program stores each as
    B2 B1 B0 -A2 -A1. The 0 dB entry is the identity filter, exactly flat."""
    out = b""
    for i in range(STEPS):
        b0, b1, b2, a1, a2 = (1, 0, 0, 0, 0) if i == FLAT else biquad(f, (i - FLAT) * step_db, fs)
        out += b"".join(fixed(x) for x in (b2, b1, b0, -a2, -a1))
    return out


def balance_tables():
    """Per-channel gain for each balance index: one side fades to silence as the
    balance moves away from it, as alsa-ohceq does."""
    left = right = b""
    for i in range(STEPS):
        bal = (i - FLAT) / FLAT  # -1 .. 1
        left += fixed(1 - bal if bal > 0 else 1)
        right += fixed(1 + bal if bal < 0 else 1)
    return left, right


def chunk(tag, body):
    head = struct.pack("<III", 12 + len(body), tag, 0)  # rate mask 0: every rate
    pad = b"\xaa" * (-len(body) % 4)
    return head + body + pad


def data(at, words):
    return chunk(TAG_DATA, struct.pack("<H", at) + words)


def control(name, at):
    return chunk(TAG_CONTROL, struct.pack("<HHH", 0, at, 4) + name.encode("ascii"))


def build(profile):
    fs = profile["rate"]
    out = []
    for k in profile["knobs"]:
        if "filter" in k:
            out.append(data(addr(k["table"]), filter_table(k["filter"], fs, profile["step_db"])))
        if "balance" in k:
            left, right = balance_tables()
            out.append(data(addr(k["balance"]["left"]), left))
            out.append(data(addr(k["balance"]["right"]), right))
        out.append(data(addr(k["cell"]), struct.pack(">I", FLAT)))
        out.append(control(k["control"], addr(k["cell"])))
    for c in profile["cells"]:
        if "value" in c:
            out.append(data(addr(c["cell"]), struct.pack(">I", c["value"])))
        out.append(control(c["control"], addr(c["cell"])))
    out.append(chunk(TAG_RATES, struct.pack("<I", fs)))
    body = b"".join(out)
    # The kernel checks crc32(0, ...): no pre- or post-inversion.
    crc = zlib.crc32(body, 0xFFFFFFFF) ^ 0xFFFFFFFF
    return b"ADISIGM" + bytes([2]) + struct.pack("<I", crc) + body


def check(profile):
    names = [k["control"] for k in profile["knobs"]] + [c["control"] for c in profile["cells"]]
    cells = [k["cell"] for k in profile["knobs"]] + [c["cell"] for c in profile["cells"]]
    for n in names:
        # sigmadsp refuses non-printable names; ALSA caps them at 43 characters.
        if not (n.isascii() and n.isprintable() and len(n) < 44):
            raise ValueError(f"bad control name {n!r}")
    if len(set(names)) != len(names) or len({addr(c) for c in cells}) != len(cells):
        raise ValueError("duplicate control name or cell")
    ids = [k["id"] for k in profile["knobs"]]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate knob id")


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: mkfw.py <profile.json> <out.bin>")
    with open(sys.argv[1]) as f:
        profile = json.load(f)
    check(profile)
    fw = build(profile)
    with open(sys.argv[2], "wb") as f:
        f.write(fw)


if __name__ == "__main__":
    main()
