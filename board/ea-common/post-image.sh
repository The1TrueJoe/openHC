#!/usr/bin/env bash
# Buildroot post-image hook: wrap the freshly built bzImage in the CEFDK
# container so `bootkernel -b` / the netboot path will accept it.
#
# $1 = BINARIES_DIR (output/images). BR2_EXTERNAL path is exported by Buildroot.
# $2 = board name, passed via BR2_ROOTFS_POST_SCRIPT_ARGS in the board defconfig.
#
# Every EA runs CEFDK, so this step is shared, and so is the container header:
# the EA1's and the EA3's were both read off live units and are byte identical.
#
# cefdk-wrap.py GENERATES that header rather than shipping a copy. openHC used
# to carry board/ea1/cefdk-container-header.bin, and 514 of its 1408 bytes were
# Control4/Intel signature material — unredistributable in an MIT repo. The
# signature is only enforced when secure boot is on, and it is not on this
# hardware, so the generated header leaves those regions zero. See the header
# comment in tools/cefdk-wrap.py.
#
# A unit that does enforce signatures needs its own extracted header; pass it
# with --header and keep it out of this repo.
set -euo pipefail
IMAGES="$1"
BOARD="${2:-ea1}"
EXT="${BR2_EXTERNAL_OPENHC_PATH:?}"

bz="$IMAGES/bzImage"
out="$IMAGES/openhc-$BOARD-kernel.img"
[ -f "$bz" ] || { echo "post-image: no bzImage at $bz" >&2; exit 1; }

python3 "$EXT/../tools/cefdk-wrap.py" "$bz" "$out"
echo "post-image: netboot image ready -> $out"
