#!/usr/bin/env python3
"""Which shipped crates does a Cargo.lock change actually affect?

    lock-impacted-crates.py <old-lock> <new-lock> <crate>...

Prints (one per line, sorted) each <crate> whose TRANSITIVE resolved dependency
set differs between the two lockfiles: a package in its closure changed version,
source or checksum, was added/removed, or its own dependency edges changed. This
is the lockfile-level equivalent of diffing `cargo metadata` resolve graphs, with
no Rust toolchain (the CI `select` job has none, and must stay cheap).

It over-approximates by design (the lock also lists dev/target-specific deps), and
anything it cannot parse exits non-zero so the caller builds every board.
Dependency strings are "name", "name version" or "name version (source)"; the
parser is hand-rolled so it works on any python3 (tomllib is 3.11+).
"""
import re
import sys


def parse(path):
    pkgs = {}  # (name, version) -> {"src":..., "sum":..., "deps":[raw strings]}
    cur = None
    in_deps = False
    with open(path) as fh:
        for line in fh:
            line = line.rstrip("\n")
            if line == "[[package]]":
                cur = {"deps": []}
                in_deps = False
                continue
            if cur is None:
                continue
            if in_deps:
                if line.strip() == "]":
                    in_deps = False
                else:
                    m = re.match(r'\s*"([^"]+)",?$', line)
                    if not m:
                        raise ValueError("bad dependency line: %r" % line)
                    cur["deps"].append(m.group(1))
                continue
            if line == "":
                if "name" in cur:
                    pkgs[(cur["name"], cur.get("version", ""))] = cur
                    cur = None
                continue
            m = re.match(r'(\w+) = (.*)$', line)
            if not m:
                raise ValueError("bad line: %r" % line)
            k, v = m.groups()
            if k == "dependencies":
                if v.strip() == "[]":
                    continue
                if v.strip() != "[":
                    raise ValueError("unexpected dependencies form: %r" % line)
                in_deps = True
            elif k in ("name", "version", "source", "checksum"):
                cur[{"source": "src", "checksum": "sum"}.get(k, k)] = v.strip().strip('"')
    if cur is not None and "name" in cur:
        pkgs[(cur["name"], cur.get("version", ""))] = cur
    if not pkgs:
        raise ValueError("no packages parsed from %s" % path)
    return pkgs


def resolve(pkgs, dep):
    parts = dep.split(" ")
    name = parts[0]
    if len(parts) > 1:
        key = (name, parts[1])
        if key in pkgs:
            return key
    cands = [k for k in pkgs if k[0] == name]
    if len(cands) != 1:
        raise ValueError("cannot resolve dependency %r" % dep)
    return cands[0]


def closures(pkgs, roots):
    out = {}
    for r in roots:
        rk = [k for k in pkgs if k[0] == r]
        if not rk:
            out[r] = None  # not in this lockfile
            continue
        seen, todo = set(), list(rk)
        while todo:
            k = todo.pop()
            if k in seen:
                continue
            seen.add(k)
            todo.extend(resolve(pkgs, d) for d in pkgs[k]["deps"])
        # Fingerprint: identity + sources + the edges, so a rewired graph differs.
        out[r] = frozenset(
            (k[0], k[1], pkgs[k].get("src"), pkgs[k].get("sum"),
             tuple(sorted(pkgs[k]["deps"])))
            for k in seen)
    return out


def main(argv):
    old, new, crates = argv[1], argv[2], argv[3:]
    co = closures(parse(old), crates)
    cn = closures(parse(new), crates)
    for c in sorted(crates):
        if co[c] != cn[c]:
            print(c)


if __name__ == "__main__":
    try:
        main(sys.argv)
    except Exception as e:  # unparseable => caller falls back to all boards
        print("lock-impacted-crates: %s" % e, file=sys.stderr)
        sys.exit(2)
