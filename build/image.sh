#!/usr/bin/env bash
# Build a board's image natively on Linux, the same steps CI runs:
#
#   build/image.sh BOARD     (or `make native BOARD=...`)
#
#   1. build/fetch-buildroot.sh   Buildroot at the pin, patched ($BUILDROOT_DIR,
#                                 default ~/buildroot)
#   2. build/stage-daemons.sh     the openHC daemons into the rootfs overlay
#   3. build/build.sh             Buildroot -> output/build/BOARD/images
#   4. build/kexec-i686.sh        hc800 only: the stock-side static kexec
#   5. build/bundle.sh            openhc-BOARD-<version>.zip for the flasher
#
# First run: build/deps.sh (Debian/Ubuntu host packages; --kexec for hc800),
# plus rustup and node/npm. Wants ~8 GB RAM and tens of GB of disk; the first
# build is hours, later ones resume from output/ and dl/. Not on macOS: use
# `make image` (Docker) there. What CI adds on top (the EA3 DSP program, the
# EA on-panel browser sysroots, a shared EA toolchain) is optional; without it
# the image simply ships without those parts.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BOARD="${1:?usage: $0 BOARD}"
[ "$(uname -s)" = Linux ] || { echo "native builds need Linux; on this host use: make image BOARD=$BOARD" >&2; exit 1; }
export BUILDROOT_DIR="${BUILDROOT_DIR:-$HOME/buildroot}"
export BOARD
export OHC_VERSION="${OHC_VERSION:-$("$REPO/build/version.sh")}"

bash "$REPO/build/fetch-buildroot.sh" "$BUILDROOT_DIR"
bash "$REPO/build/stage-daemons.sh" "$BOARD"
bash "$REPO/build/build.sh"
case "$BOARD" in
  hc800*) bash "$REPO/build/kexec-i686.sh" "$REPO/output/build/$BOARD/images/kexec-i686-static" ;;
esac
bundle=$(bash "$REPO/build/bundle.sh" "$BOARD" "$OHC_VERSION")
echo ">> $BOARD $OHC_VERSION: $REPO/$bundle"
