#!/usr/bin/env bash
# A static i686 kexec for the VENDOR userspace: Control4 OS 3.x is i686 glibc
# from 2015-ish, whose shared libraries we neither have nor want to match, so
# static removes the question. The HC-800's first install starts openHC from
# the running Control4 image with it; every hc800 bundle carries one.
#
#   build/kexec-i686.sh OUT       needs gcc -m32 (build/deps.sh --kexec)
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?usage: $0 OUT}"
# kernel.org release. Pinned: this is a binary someone will run as root on
# hardware with no recovery console.
KEXEC_VERSION=2.0.31
KEXEC_SHA256=8a8f350ddc66e1c905a3ab525a7e9ba96c81e04e70ef69397b0155b67b922c31

work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
echo 'int main(void){return 0;}' > "$work/t.c"
# file 5.46 (Ubuntu 26.04) says "Intel i386" where older ones said "Intel 80386".
gcc -m32 -static -o "$work/t" "$work/t.c" 2>/dev/null && file "$work/t" | grep -qE "Intel (80386|i386)" \
  || { echo "gcc -m32 -static does not produce i386 binaries: build/deps.sh --kexec" >&2; exit 1; }

tarball="$work/kexec-tools-$KEXEC_VERSION.tar.xz"
bash "$REPO/.github/scripts/fetch.sh" "$tarball" "$KEXEC_SHA256" \
  "https://cdn.kernel.org/pub/linux/utils/kernel/kexec/kexec-tools-$KEXEC_VERSION.tar.xz" \
  "https://mirrors.edge.kernel.org/pub/linux/utils/kernel/kexec/kexec-tools-$KEXEC_VERSION.tar.xz"
tar -C "$work" -xf "$tarball"
# --host is what selects kexec's i386 loaders; x86_64 with -m32 would compile
# the wrong set.
( cd "$work/kexec-tools-$KEXEC_VERSION" && ./configure --host=i686-pc-linux-gnu \
    --build=x86_64-pc-linux-gnu CC="gcc -m32" LDFLAGS="-static" \
    --without-zlib --without-lzma --without-xen >/dev/null && make -j"$(nproc)" >/dev/null )
mkdir -p "$(dirname "$OUT")"
cp "$work/kexec-tools-$KEXEC_VERSION/build/sbin/kexec" "$OUT"

file "$OUT"
file "$OUT" | grep -qE "ELF 32-bit.*Intel (80386|i386)" || { echo "not a 32-bit x86 binary" >&2; exit 1; }
file "$OUT" | grep -q "statically linked" || { echo "NOT STATIC: it would need the vendor's glibc" >&2; exit 1; }
# A dynamic section would mean a runtime dependency we cannot satisfy.
if readelf -d "$OUT" 2>/dev/null | grep -q NEEDED; then
  echo "NEEDED entries: not truly static" >&2; exit 1
fi
ls -l "$OUT"
