#!/usr/bin/env bash
# In-container Buildroot builder. Invoked by build/Dockerfile during
# `docker build` (see the Makefile `image` target). Runs against the Buildroot
# tree at $BUILDROOT_DIR, writing to cache-mounted dl/ and output/ so nothing
# touches the macOS bind mount (which crashed Docker Desktop's virtiofs under
# Buildroot's heavy package I/O).
#
#   ./build.sh            # configure + build the image (default)
#   ./build.sh <target>   # any Buildroot make target
#
# Job count auto-caps by RAM (~1.5GB/job, min with CPU count). Override BR2_JLEVEL.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BUILDROOT_DIR="${BUILDROOT_DIR:-/opt/buildroot}"
OUT="${BR2_OUTPUT_DIR:-$REPO/output/build}"
DL="${BR2_DL_DIR:-$REPO/dl}"

cpu=$(nproc)
memkb=$(awk '/^MemTotal:/{print $2}' /proc/meminfo)
memjobs=$(( memkb / 1500000 )); [ "$memjobs" -lt 1 ] && memjobs=1
JOBS="${BR2_JLEVEL:-$cpu}"
[ "$memjobs" -lt "$JOBS" ] && JOBS="$memjobs"
echo ">> BR2_JLEVEL=$JOBS (cpu=$cpu, mem-safe=$memjobs)"

[ -f "$BUILDROOT_DIR/Makefile" ] || { echo "no Buildroot at $BUILDROOT_DIR" >&2; exit 1; }
mkdir -p "$OUT" "$DL"

M=(make -C "$BUILDROOT_DIR" O="$OUT" BR2_EXTERNAL="$REPO/board/ea1"
   BR2_DL_DIR="$DL" BR2_JLEVEL="$JOBS")

"${M[@]}" ea1_defconfig
# Kernel bring-up iterates on both the config fragment AND the patch set
# (board/ea1/patches/linux/). Buildroot applies patches only at EXTRACT time and
# won't re-extract a cached source, so a plain reconfigure silently ignores new
# patches. linux-dirclean wipes the extracted tree; the next build re-extracts,
# re-applies all patches, reconfigures from the fragment, and rebuilds. Drop this
# once the kernel + patches stabilize (it forces a full kernel rebuild each run).
"${M[@]}" linux-dirclean
"${M[@]}" "${@:-all}"

echo ">> image: $OUT/images/openhc-ea1-kernel.img"
