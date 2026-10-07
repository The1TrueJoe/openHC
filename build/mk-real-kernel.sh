#!/usr/bin/env bash
# Build the EA "real" (fat) kernel — the one kexec'd from p1, with the large
# subsystems (DRM/SGX, …) built IN instead of as modules. See
# board/ea/common/linux/real-kernel.fragment and the KERNEL SIZE BUDGET note in
# board/ea/common/linux/common.fragment for why EA boot is two kernels.
#
# This is a SECOND, out-of-tree compile of the SAME kernel source Buildroot
# already extracted, patched and mirrored openHC's drivers + the sgx545ce
# submodule into. It reuses that source and Buildroot's own cross toolchain, so
# it cannot drift from the bootstrap kernel's source or compiler; only the
# .config differs (bootstrap config + real-kernel.fragment, olddefconfig'd).
#
#   mk-real-kernel.sh <OUT> <REPO> [OUTFILE]
#     OUT     = output/build/<board>  (Buildroot per-board output; has build/linux-*, host/)
#     REPO    = repo root (to find board/ea/common/linux/real-kernel.fragment)
#     OUTFILE = where to write the fat bzImage (default <OUT>/images/bzImage-real)
#
# It does NOT touch the bootstrap kernel's build tree or the image — it writes
# one extra bzImage. Wiring it into the p1 rootfs is the caller's job.
set -euo pipefail

OUT="${1:?usage: mk-real-kernel.sh <OUT> <REPO> [OUTFILE]}"
REPO="${2:?}"
OUTFILE="${3:-$OUT/images/bzImage-real}"

FRAG="$REPO/board/ea/common/linux/real-kernel.fragment"
[ -f "$FRAG" ] || { echo "mk-real-kernel: no $FRAG" >&2; exit 1; }

# The kernel source Buildroot built (patched + openHC drivers mirrored in).
SRC=$(ls -d "$OUT"/build/linux-*/ 2>/dev/null | grep -vE 'linux-(headers|tools|real)' | head -1)
SRC="${SRC%/}"
[ -n "$SRC" ] && [ -f "$SRC/.config" ] || {
	echo "mk-real-kernel: no built kernel source with a .config under $OUT/build/linux-*" >&2
	exit 1
}
echo "mk-real-kernel: source $SRC"

# Buildroot's cross toolchain. x86_64 target (BR2_x86_64); the tuple is stable.
HOSTBIN="$OUT/host/bin"
TUPLE="x86_64-buildroot-linux-gnu"
[ -x "$HOSTBIN/$TUPLE-gcc" ] || {
	# fall back to whatever x86_64 cross-gcc the host dir has
	TUPLE=$(ls "$HOSTBIN"/*-linux-*-gcc 2>/dev/null | head -1 | xargs -r basename | sed 's/-gcc$//')
	[ -n "$TUPLE" ] || { echo "mk-real-kernel: no cross gcc in $HOSTBIN" >&2; exit 1; }
}
echo "mk-real-kernel: toolchain $HOSTBIN/$TUPLE-gcc"

RBUILD="$OUT/build/linux-real"
mkdir -p "$RBUILD"
export PATH="$HOSTBIN:$PATH"
MAKE=(make -C "$SRC" O="$RBUILD" ARCH=x86_64 CROSS_COMPILE="$TUPLE-" \
      HOSTCC="$HOSTBIN/$TUPLE-gcc" -j"$(nproc 2>/dev/null || echo 4)")

# Start from the bootstrap kernel's resolved .config so we inherit every EA
# fragment, then layer real-kernel.fragment and let olddefconfig settle deps.
cp "$SRC/.config" "$RBUILD/.config"
if [ -x "$SRC/scripts/kconfig/merge_config.sh" ]; then
	( cd "$SRC" && ARCH=x86_64 ./scripts/kconfig/merge_config.sh -O "$RBUILD" -m "$RBUILD/.config" "$FRAG" )
else
	cat "$FRAG" >> "$RBUILD/.config"
fi
"${MAKE[@]}" olddefconfig

# Sanity: the whole point is these must be built IN (=y), not modules.
for sym in CONFIG_DRM CONFIG_SGX545_CE; do
	if ! grep -q "^$sym=y" "$RBUILD/.config"; then
		echo "mk-real-kernel: WARNING $sym is not =y in the real kernel (.config says: $(grep "$sym" "$RBUILD/.config" || echo unset))" >&2
	fi
done

echo "mk-real-kernel: building fat bzImage (DRM/SGX built in) ..."
"${MAKE[@]}" bzImage

BZ="$RBUILD/arch/x86/boot/bzImage"
[ -f "$BZ" ] || { echo "mk-real-kernel: no bzImage produced at $BZ" >&2; exit 1; }
mkdir -p "$(dirname "$OUTFILE")"
cp -f "$BZ" "$OUTFILE"
echo "mk-real-kernel: real kernel -> $OUTFILE ($(wc -c < "$OUTFILE") bytes, no size limit — kexec'd)"
