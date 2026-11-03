#!/usr/bin/env python3
"""Turn a SigmaStudio(+) export of the ADAU1451 program into mkfw.py's input.

    export2program.py export-folder program.txt [--params params.json]

The export folder is what SigmaStudio+'s Export writes (dsp-program.yml runs
it in CI). Its <project>_<schematic>.h holds the default download: every write
SigmaStudio makes to bring the part up, in order, each an address and a byte
array. program.txt gets the ones into memory (program, DM0, DM1), in that
order, as "@<address>" blocks, which is mkfw.py's --program format; the
control-register writes are left out, because the profile's init owns those.

--params also writes the export's parameter map (<schematic>.param) as JSON,
{module: {parameter: {"address": n, "memory": "DM0"}}}, so the profile can
name a cell by its module and parameter instead of a bare address.

Standard library only; the build host's python3 runs it.
"""
import argparse
import glob
import json
import os
import re

from mkfw import MEMORY


def schematic_header(folder):
    """The export's one program header: <project>_<schematic>.h, not the
    _PARAM.h or _Defines.h that sit beside it."""
    hs = [h for h in glob.glob(os.path.join(folder, "*.h"))
          if not re.search(r"_(PARAM|Defines)\.h$", h)]
    if len(hs) != 1:
        raise ValueError(f"{folder}: want one program header, found {hs or 'none'}")
    return hs[0]


def default_download(path):
    """The (address, bytes) of each SIGMA_WRITE_REGISTER_BLOCK in the default
    download, in the order it makes them. Delays are left out."""
    text = open(path, encoding="utf-8", errors="replace").read()
    defines = dict(re.findall(r"^#define\s+(\w+)\s+(\d+)\s*$", text, re.M))
    arrays = {name: bytes(int(b, 16) for b in re.findall(r"0x([0-9A-Fa-f]{2})", body))
              for name, body in re.findall(r"ADI_REG_TYPE\s+(\w+)\s*\[[^\]]*\]\s*=\s*\{([^}]*)\}", text)}
    body = re.search(r"void\s+default_download_\w+\s*\(\s*\)\s*\{(.*?)^\}", text, re.S | re.M)
    if not body:
        raise ValueError(f"{path}: no default_download function")
    writes = []
    for at, size, data in re.findall(
            r"SIGMA_WRITE_REGISTER_BLOCK\(\s*\w+\s*,\s*(\w+)\s*,\s*(\w+)\s*,\s*(\w+)\s*\)", body.group(1)):
        b = arrays[data]
        if int(defines[size]) != len(b):
            raise ValueError(f"{path}: {data} is {len(b)} bytes, {size} says {defines[size]}")
        writes.append((int(defines[at]), b))
    return writes


def memory_blocks(writes):
    """The writes that land in program, DM0 or DM1, program first."""
    def region(at, data):
        for rank, (_, base, words) in enumerate(MEMORY):
            if base <= at and at + len(data) // 4 <= base + words:
                return rank
        return None
    blocks = [(region(at, d), at, d) for at, d in writes if not len(d) % 4]
    blocks = [b for b in blocks if b[0] is not None]
    if not any(rank == 0 for rank, _, _ in blocks):
        raise ValueError("the export writes no program memory")
    return [(at, d) for _, at, d in sorted(blocks, key=lambda b: b[0])]


def parameters(folder):
    """{module: {parameter: {address, memory}}} from the export's .param."""
    ps = glob.glob(os.path.join(folder, "*.param"))
    if len(ps) != 1:
        raise ValueError(f"{folder}: want one .param, found {ps or 'none'}")
    out = {}
    for rec in re.split(r"\n\s*\n", open(ps[0], encoding="utf-8", errors="replace").read()):
        f = dict((k.strip(), v.strip()) for k, v in re.findall(r"^([^=\n]+?)\s*=\s*(.*)$", rec, re.M))
        if "Module Name" in f and "Parameter Address" in f:
            out.setdefault(f["Module Name"], {})[f["Parameter Name"]] = {
                "address": int(f["Parameter Address"]), "memory": f.get("Data Memory", "")}
    return out


def main():
    ap = argparse.ArgumentParser(description="SigmaStudio(+) export to mkfw.py --program input.")
    ap.add_argument("export")
    ap.add_argument("out")
    ap.add_argument("--params", help="also write the parameter map here, as JSON")
    args = ap.parse_args()
    h = schematic_header(args.export)
    with open(args.out, "w") as f:
        f.write(f"# from {os.path.basename(h)}: the memory writes of its default download\n")
        for at, d in memory_blocks(default_download(h)):
            f.write(f"@0x{at:04x}\n")
            for i in range(0, len(d), 4):
                f.write(" ".join(f"{b:02x}" for b in d[i:i + 4]) + "\n")
    if args.params:
        with open(args.params, "w") as f:
            json.dump(parameters(args.export), f, indent=1, sort_keys=True)
            f.write("\n")


if __name__ == "__main__":
    main()
