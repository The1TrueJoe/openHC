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
# Kernel config still iterates during bring-up (fragment changes, rootfs type),
# and Buildroot won't rebuild a cached kernel just because the fragment changed.
# Force a reconfigure+rebuild of linux so config edits actually take. Drop this
# line once the kernel config stabilizes.
"${M[@]}" linux-reconfigure
"${M[@]}" "${@:-all}"

echo ">> image: $OUT/images/openhc-ea1-kernel.img"
