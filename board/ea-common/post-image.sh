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
# comment in flasher/wrap.py.
#
# A unit that does enforce signatures needs its own extracted header; pass it
# with --header and keep it out of this repo.
set -euo pipefail

# Say where we died. Buildroot prints only "Error 1" for a failed post-image
# script, and with `set -e` an unset variable or a failed test aborts with no
# message of its own -- which is exactly what happened here for three builds
# running: "Executing post-image script ..." followed immediately by Error 1 and
# not one line of explanation.
trap 'echo "post-image: FAILED at line $LINENO: $BASH_COMMAND" >&2' ERR
echo "post-image: start (args: $*)"
echo "post-image: BR2_EXTERNAL_OPENHC_PATH=${BR2_EXTERNAL_OPENHC_PATH:-<UNSET>}"

IMAGES="$1"
BOARD="${2:-ea1}"
EXT="${BR2_EXTERNAL_OPENHC_PATH:?}"

bz="$IMAGES/bzImage"
out="$IMAGES/openhc-$BOARD-kernel.img"
[ -f "$bz" ] || { echo "post-image: no bzImage at $bz" >&2; exit 1; }

# Container layout lives in the flasher so the build and the installer cannot
# drift apart. Find it wherever this is running: the build container installs
# it on PATH, a host build has it under flasher/target/.
find_flasher() {
	[ -n "${OHC_FLASH:-}" ] && { echo "$OHC_FLASH"; return 0; }
	command -v ohc-flash 2>/dev/null && return 0
	for p in "$EXT/../flasher/target/release/ohc-flash" \
	         "$EXT/../flasher/target/debug/ohc-flash"; do
		[ -x "$p" ] && { echo "$p"; return 0; }
	done
	# EXPLICIT SUCCESS ON FINDING NOTHING, and this line is load bearing.
	# Without it the function's status is the last failed `[ -x ]`, so
	# `flash=$(find_flasher)` inherits 1 and `set -e` kills the build — which is
	# the exact opposite of the "must NOT be able to fail the build" contract
	# written immediately below, and it is how ea1-v1 died in CI: post-image
	# aborted at this assignment, before it could print its own warning.
	return 0
}
# The wrap is for the NETBOOT image only. It must NOT be able to fail the
# build, because the two artifacts the installer actually needs -- the tiny
# boot-init and the staged rootfs, built below -- are independent of it, and
# `set -e` was throwing them away over it. Observed exactly that: post-image
# aborted with no message at all and the build produced no boot-init.cpio.gz,
# which made a persistent install impossible for a reason that looked like a
# kernel problem.
#
# install_self() wraps the kernel itself (image::container), so a missing
# openhc-*-kernel.img costs only the netboot path, not the install.
flash=$(find_flasher)
if [ -z "$flash" ]; then
	echo "post-image: WARNING no ohc-flash binary; skipping netboot wrap." >&2
	echo "  (install_self wraps the kernel itself, so this only affects netboot)" >&2
	echo "  build it with: cargo build --release -p ohc-flash-cli, or set OHC_FLASH=" >&2
elif "$flash" wrap "$bz" "$out"; then
	echo "post-image: netboot image ready -> $out"
else
	echo "post-image: WARNING ohc-flash wrap failed (rc=$?); netboot image not built." >&2
fi

# ---- tiny self-installer boot-initramfs + staged rootfs ---------------------
# The autoscript loads the kernel + this tiny initramfs (busybox + one /init);
# on first boot /init self-installs p1 from a gzipped rootfs staged in the eMMC
# gap, then switch_roots to p1 on every boot after. So a normal boot unpacks
# ~1 MB instead of the whole rootfs, and the flasher writes everything in one
# stage. TARGET_DIR is the images dir's sibling in this build layout.
TARGET="$IMAGES/../target"
BOOTINIT="$EXT/ea-common/boot-init/init"
if [ -d "$TARGET" ] && [ -f "$BOOTINIT" ]; then
	"$EXT/../build/mk-boot-init.sh" "$TARGET" "$IMAGES/boot-init.cpio.gz" "$BOOTINIT" \
		|| echo "post-image: WARNING boot-init build failed (tiny-init install unavailable)" >&2
fi

# Stage image: a gzipped copy of the ext2 rootfs, which the tiny /init streams
# from the eMMC gap straight to p1 (gunzip | dd) — the 512 MB image never lands
# in RAM. rootfs.ext2 is mostly zeros, so this is small.
if [ -f "$IMAGES/rootfs.ext2" ]; then
	gzip -9 -c "$IMAGES/rootfs.ext2" > "$IMAGES/rootfs.ext2.gz"
	echo "post-image: staged rootfs -> rootfs.ext2.gz ($(wc -c < "$IMAGES/rootfs.ext2.gz") bytes)"
fi
