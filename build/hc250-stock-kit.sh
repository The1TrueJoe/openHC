#!/usr/bin/env bash
# The HC-250's stock-side kit, into the board's images/ for the flasher:
# ohc_kexec.ko (built against Control4's GPL 2.6.32 headers with Buildroot's own
# cross gcc), ohc-kexec.sh and ohc-boot-env.sh. Stock has no kexec and its env
# must be written by its own fw_setenv; see board/hc250/stock-kexec.
#
#   build/hc250-stock-kit.sh <images-dir> [buildroot-host-dir]
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
IMAGES=${1:?usage: $0 <images-dir> [host-dir]}
HOST=${2:-$REPO/output/build/hc250/host}
KIT=$REPO/board/hc250/stock-kexec
# The GPL tarball (61 MB) is kept under dl/, which CI caches with Buildroot's downloads.
OUT=$REPO/dl/hc250-stock-kexec

CC=${CC:-$HOST/bin/arm-buildroot-linux-musleabihf-gcc} bash "$KIT/build.sh" "$OUT"
install -m 0644 "$OUT/ohc_kexec.ko" "$IMAGES/ohc_kexec.ko"
install -m 0755 "$KIT/ohc-kexec.sh" "$IMAGES/ohc-kexec.sh"
install -m 0755 "$KIT/ohc-boot-env.sh" "$IMAGES/ohc-boot-env.sh"
echo "hc250-stock-kit: ohc_kexec.ko ohc-kexec.sh ohc-boot-env.sh -> $IMAGES"
