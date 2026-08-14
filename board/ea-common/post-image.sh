#!/usr/bin/env bash
# Buildroot post-image hook: wrap the freshly built bzImage in the CEFDK
# container so `bootkernel -b` / the netboot path will accept it.
#
# $1 = BINARIES_DIR (output/images). BR2_EXTERNAL path is exported by Buildroot.
# $2 = board name, passed via BR2_ROOTFS_POST_SCRIPT_ARGS in the board defconfig.
#
# Every EA runs CEFDK, so this step is shared. What is NOT shared is the
# container header: only the EA1's has been extracted from a stock image. A
# board without its own header falls back to the EA1's with a warning, because
# the two are very likely identical (same SoC, same bootloader family) — but
# "very likely" is not "verified", and a bad header is a failed boot, so say so.
set -euo pipefail
IMAGES="$1"
BOARD="${2:-ea1}"
EXT="${BR2_EXTERNAL_OPENHC_PATH:?}"

bz="$IMAGES/bzImage"
out="$IMAGES/openhc-$BOARD-kernel.img"
[ -f "$bz" ] || { echo "post-image: no bzImage at $bz" >&2; exit 1; }

hdr="$EXT/$BOARD/cefdk-container-header.bin"
if [ ! -f "$hdr" ]; then
    hdr="$EXT/ea1/cefdk-container-header.bin"
    echo "post-image: WARNING — no CEFDK header captured for $BOARD;" >&2
    echo "post-image:   falling back to the EA1 header. Unverified on $BOARD." >&2
fi

python3 "$EXT/../tools/cefdk-wrap.py" "$bz" "$out" "$hdr"
echo "post-image: netboot image ready -> $out"
