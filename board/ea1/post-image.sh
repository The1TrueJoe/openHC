#!/usr/bin/env bash
# Buildroot post-image hook: wrap the freshly built bzImage in the CEFDK
# container so `bootkernel -b` / the netboot path will accept it.
#
# $1 = BINARIES_DIR (output/images). BR2_EXTERNAL path is exported by Buildroot.
set -euo pipefail
IMAGES="$1"
EXT="${BR2_EXTERNAL_OPENHC_EA1_PATH:?}"

bz="$IMAGES/bzImage"
out="$IMAGES/openhc-ea1-kernel.img"
[ -f "$bz" ] || { echo "post-image: no bzImage at $bz" >&2; exit 1; }

python3 "$EXT/../../tools/cefdk-wrap.py" "$bz" "$out"
echo "post-image: netboot image ready -> $out"
