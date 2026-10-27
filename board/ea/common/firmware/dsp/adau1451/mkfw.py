#!/usr/bin/env python3
"""Build openHC's SigmaDSP firmware file for an ADAU1451 from a profile.

    mkfw.py ea3.json adau1451-ea3.bin [--program program.txt [--params params.json]]

The output is the kernel's own SigmaDSP firmware format (sound/soc/codecs/
sigmadsp.c, "ADISIGM" version 2, mainline chunk tags), so the stock sigmadsp
core loads it and turns it into ALSA controls; there is no openHC loader. It
holds three kinds of chunk:

  data      bytes to write at an address, in file order: with a program, first
            the profile's control-register init (one 16-bit register a chunk),
            then the program memory and the data memory that came with it;
            then every knob's 49-entry table, computed here from the
            profile's designs, and the cells' values
  control   one ALSA control per knob and cell, 4 bytes at the cell's address
  rates     the one sample rate the program runs at

Without --program the file holds only the parameters, exactly as before a
program existed: no register init, no program memory.

A knob or cell names its address with "cell", or, for a program built in CI,
with "param": "<block label>.<parameter>", looked up in the params.json that
export2program.py writes beside the program (--params). A profile's init may
be {"from": "other.json"}: that profile's init, so two profiles for the same
board share one register setup.

A profile's "version" ({"param" or "cell", "value"}) is the program's version
cell and what it holds; --expect-version "<cell> <value>" (the device tree's
control4,program-version) fails the build if the two disagree, so a design
change that moves the cell cannot ship a firmware the driver then stops.

The program file is plain text: a line "@<address>" starts a block, and the
hex bytes after it (whitespace or commas between them, "0x" optional, "#"
comments) are written there, 4 bytes a word, big-endian, as the part takes
them. That is the address and byte array of each memory write in a
SigmaStudio(+) export (Program_Data, Param_Data, DM1_DATA_Data). Only memory:
program (0xc000..), DM0 (0x0000..) and DM1 (0x6000..); control registers are
the profile's init.

Standard library only; the build host's python3 runs it.
"""
import argparse
import json
import math
import os
import struct
import zlib

# The program's tables are 49 entries, the middle one neutral; only the step
# between entries is the profile's to choose.
STEPS = 49
FLAT = 24

TAG_DATA, TAG_CONTROL, TAG_RATES = 0, 1, 2

# The ADAU1451's memory map (datasheet Table 62), in 32-bit words, in the order
# the part is loaded: program, then the data memories it runs on.
MEMORY = (("program", 0xC000, 8192), ("DM0", 0x0000, 16384), ("DM1", 0x6000, 16384))
CTRL = range(0xF000, 0xF900)
# Sequencing registers: the driver writes these itself, around the init.
DRIVER_OWNS = {0xF400: "HIBERNATE", 0xF402: "START_CORE", 0xF403: "KILL_CORE",
               0xF890: "SOFT_RESET"}


def addr(s):
    return int(s, 0)


def word(x):
    """A profile word: an 8.24 number, or a "0x..." string taken as raw bits."""
    if isinstance(x, str):
        return struct.pack(">I", int(x, 16))
    return fixed(x)


def words(xs):
    return b"".join(word(x) for x in xs)


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


def control(name, at, nbytes=4):
    return chunk(TAG_CONTROL, struct.pack("<HHH", 0, at, nbytes) + name.encode("ascii"))


def init_writes(profile):
    """The profile's control-register init as (register, value) pairs, in order."""
    out = []
    for w in profile["init"]["writes"]:
        reg, value = addr(w["reg"]), int(w["value"], 0)
        for r in range(reg, reg + w.get("count", 1)):
            if r not in CTRL or r in DRIVER_OWNS:
                raise ValueError(f"init: 0x{r:04x} is not a register the init may write")
            if not 0 <= value <= 0xFFFF:
                raise ValueError(f"init: 0x{r:04x} = {w['value']} does not fit 16 bits")
            out.append((r, value))
    return out


def load_program(path):
    """The program file's memory blocks as (address, bytes), program memory
    first, then DM0, then DM1, each in file order."""
    blocks = []
    with open(path) as f:
        for n, line in enumerate(f, 1):
            line = line.split("#", 1)[0].strip()
            if line.startswith("@"):
                blocks.append((addr(line[1:]), bytearray()))
            elif line:
                if not blocks:
                    raise ValueError(f"{path}:{n}: data before the first @address")
                for t in line.replace(",", " ").split():
                    h = t[2:] if t.lower().startswith("0x") else t
                    if not 1 <= len(h) <= 2:
                        raise ValueError(f"{path}:{n}: {t!r} is not one byte")
                    blocks[-1][1].append(int(h, 16))

    def region(at, data):
        for rank, (_, base, words) in enumerate(MEMORY):
            if base <= at and at + len(data) // 4 <= base + words:
                return rank
        raise ValueError(f"{path}: block at 0x{at:04x} ({len(data)} bytes) is not inside "
                         "program, DM0 or DM1 memory")

    for at, data in blocks:
        if not data or len(data) % 4:
            raise ValueError(f"{path}: block at 0x{at:04x} is not whole 32-bit words")
    blocks.sort(key=lambda b: region(*b))
    if not blocks or region(*blocks[0]) != 0:
        raise ValueError(f"{path}: no program memory")
    return [(at, bytes(data)) for at, data in blocks]


def load_profile(path, params=None):
    """The profile, its init taken from another profile if it says so, and
    every "param" name resolved to its cell through params.json."""
    with open(path) as f:
        profile = json.load(f)
    src = profile["init"].get("from")
    if src:
        with open(os.path.join(os.path.dirname(path), src)) as f:
            profile["init"] = json.load(f)["init"]
    version = [profile["version"]] if "version" in profile else []
    for e in profile["knobs"] + profile["cells"] + version:
        if "param" not in e:
            continue
        if params is None:
            raise ValueError(f"{e.get('control', 'version')}: names {e['param']!r}, but there is no --params")
        block, _, name = e["param"].partition(".")
        p = params.get(block, {}).get(name)
        if p is None:
            raise ValueError(f"{e.get('control', 'version')}: the program has no parameter {e['param']!r}")
        if p["memory"] != "DM0":
            raise ValueError(f"{e.get('control', 'version')}: {e['param']} is in {p['memory']}; only DM0 is resolved")
        e["cell"] = f"0x{p['address']:04x}"
    return profile


def build(profile, program=None):
    fs = profile["rate"]
    out = []
    if program:
        # The order the part needs: clocks and ports (the driver waits for the
        # PLL to lock part-way), then the program and its data. The parameters
        # below then land on top of that data.
        out += [data(r, struct.pack(">H", v)) for r, v in init_writes(profile)]
        out += [data(at, d) for at, d in program]
    for k in profile["knobs"]:
        if "choices" in k:
            # A selector: its first choice at boot, the control as wide as a choice.
            boot = words(k["choices"][0]["words"])
            out.append(data(addr(k["cell"]), boot))
            out.append(control(k["control"], addr(k["cell"]), len(boot)))
            continue
        if "filter" in k:
            out.append(data(addr(k["table"]), filter_table(k["filter"], fs, profile["step_db"])))
        if "balance" in k:
            left, right = balance_tables()
            out.append(data(addr(k["balance"]["left"]), left))
            out.append(data(addr(k["balance"]["right"]), right))
        out.append(data(addr(k["cell"]), struct.pack(">I", FLAT)))
        out.append(control(k["control"], addr(k["cell"])))
    for c in profile["cells"]:
        if "words" in c:
            w = words(c["words"])
            out.append(data(addr(c["cell"]), w))
            out.append(control(c["control"], addr(c["cell"]), len(w)))
            continue
        if "value" in c:
            out.append(data(addr(c["cell"]), struct.pack(">I", c["value"])))
        out.append(control(c["control"], addr(c["cell"])))
    out.append(chunk(TAG_RATES, struct.pack("<I", fs)))
    body = b"".join(out)
    # The kernel checks crc32(0, ...): no pre- or post-inversion.
    crc = zlib.crc32(body, 0xFFFFFFFF) ^ 0xFFFFFFFF
    return b"ADISIGM" + bytes([2]) + struct.pack("<I", crc) + body


def check_version(profile, expect):
    """The device tree's "<cell> <value>" against the profile's version."""
    v = profile.get("version")
    if v is None:
        raise ValueError("--expect-version, but the profile has no version")
    cell, value = (int(x, 0) for x in expect.split())
    have = (addr(v["cell"]), int(v["value"], 0) if isinstance(v["value"], str) else v["value"])
    if have != (cell, value):
        raise ValueError(f"the device tree checks cell 0x{cell:04x} = 0x{value:x}, but the program's "
                         f"version is cell 0x{have[0]:04x} = 0x{have[1]:x}: update "
                         "control4,program-version in ea-audio-dsp.dtsi")


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
    for k in profile["knobs"]:
        if "choices" in k and len({len(c["words"]) for c in k["choices"]}) != 1:
            raise ValueError(f"{k['id']}: choices of different widths")


def main():
    ap = argparse.ArgumentParser(description="Build an ADAU1451 SigmaDSP firmware file.")
    ap.add_argument("profile")
    ap.add_argument("out")
    ap.add_argument("--program", help="the DSP program to load (see above); none: parameters only")
    ap.add_argument("--params", help="the program's params.json, to resolve \"param\" names")
    ap.add_argument("--expect-version", metavar='"CELL VALUE"',
                    help="the device tree's control4,program-version, checked against the profile's version")
    args = ap.parse_args()
    params = None
    if args.params:
        with open(args.params) as f:
            params = json.load(f)
    profile = load_profile(args.profile, params)
    check(profile)
    if args.expect_version:
        check_version(profile, args.expect_version)
    init_writes(profile)
    program = load_program(args.program) if args.program else None
    fw = build(profile, program)
    with open(args.out, "wb") as f:
        f.write(fw)


if __name__ == "__main__":
    main()
