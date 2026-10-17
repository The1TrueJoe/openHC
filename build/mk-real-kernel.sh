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

export PATH="$HOSTBIN:$PATH"
# HOSTCC is the BUILD machine's gcc, left PLAIN (no -isystem/-L into Buildroot's
# host/lib). The kernel's host tools link system libs: objtool needs libelf
# (gelf.h) and certs/extract-cert needs libssl + the system libc. Pointing HOSTCC
# at $OUT/host/lib made extract-cert's link pull a broken libc.so from there
# ("ld: cannot find /lib64/libc.so.6"); and Buildroot only gets away with that
# because its OWN kernel build leaves extract-cert out. So the workflow installs
# libelf-dev + libssl-dev on the runner and we use them system-wide instead.
# (CROSS gcc from host/bin still builds the target objects.)
HOSTGCC="${HOSTGCC:-/usr/bin/gcc}"; command -v "$HOSTGCC" >/dev/null 2>&1 || HOSTGCC=gcc
#
# certs/extract-cert is the ONE host tool that trips here. It links libcrypto,
# found via $(HOSTPKG_CONFIG). With $HOSTBIN on PATH that resolves to Buildroot's
# host pkg-config, which returns `-L<host/lib>` where libcrypto.so sits next to a
# GNU-ld `libc.so` script that GROUPs /lib64/libc.so.6 — a path that does not
# exist on the runner, hence "ld: cannot find /lib64/libc.so.6". The trusted
# keyring that pulls extract-cert in is SELECTED (SYSTEM_DATA_VERIFICATION), so it
# cannot be config'd off. The fix is to point extract-cert at the runner's own
# pkg-config + openssl (libssl-dev) with a clean pkg-config env, so it links the
# system libcrypto and system libc instead of Buildroot's.
HOSTPC=/usr/bin/pkg-config; command -v "$HOSTPC" >/dev/null 2>&1 || HOSTPC=pkg-config
MAKE=(make -C "$SRC" ARCH=x86_64 CROSS_COMPILE="$HOSTBIN/$TUPLE-" \
      HOSTCC="$HOSTGCC" HOSTPKG_CONFIG="$HOSTPC" \
      PKG_CONFIG="$HOSTPC" PKG_CONFIG_LIBDIR= PKG_CONFIG_PATH= PKG_CONFIG_SYSROOT_DIR= \
      KCFLAGS="-Wno-attribute-alias" WERROR=0 \
      -j"$(nproc 2>/dev/null || echo 4)")

# IN-TREE build. Buildroot builds the kernel in-tree, so $SRC is "dirty"; the
# kernel then REFUSES an out-of-tree `O=` build ("outputmakefile Error 1"). We
# cannot O= our way out, so we mrproper and rebuild the fat kernel in the same
# tree. SAFE in sgx-module.yml: the bootstrap bzImage + pvrsrvkm.ko are already
# harvested to out/ BEFORE this runs, and the trim step re-extracts linux next
# build anyway. mrproper clears .config + build artifacts but NOT the mirrored
# openHC driver sources / Makefile edits / the sgx545ce submodule (those are
# source, not generated), so the fat build still sees them.
SLOTCFG=$(mktemp)
cp "$SRC/.config" "$SLOTCFG"
echo "mk-real-kernel: mrproper (clean in-tree build for the fat config) ..."
"${MAKE[@]}" mrproper >/dev/null

# Bootstrap's resolved .config as the base, layer real-kernel.fragment, settle.
cp "$SLOTCFG" "$SRC/.config"
if [ -x "$SRC/scripts/kconfig/merge_config.sh" ]; then
	( cd "$SRC" && ARCH=x86_64 ./scripts/kconfig/merge_config.sh -m "$SRC/.config" "$FRAG" )
else
	cat "$FRAG" >> "$SRC/.config"
fi
"${MAKE[@]}" olddefconfig

# Turn module signing OFF (no signing key, no signed-module enforcement — secure
# boot is off on this hardware). This one DOES stick. The trusted keyring and
# SYSTEM_DATA_VERIFICATION cannot be turned off from here — they are SELECTed, not
# user-settable — so certs/extract-cert is still built; it links fine now that
# HOSTPKG_CONFIG points at the runner's openssl (see the MAKE[] note above).
if [ -x "$SRC/scripts/config" ]; then
	"$SRC/scripts/config" --file "$SRC/.config" \
		-d MODULE_SIG -d MODULE_SIG_ALL -d MODULE_SIG_FORCE 2>/dev/null || true
	"${MAKE[@]}" olddefconfig
fi
echo "mk-real-kernel: cert/keyring symbols:"
grep -E 'CONFIG_(MODULE_SIG|SYSTEM_TRUSTED_KEYRING|SYSTEM_DATA_VERIFICATION)' "$SRC/.config" || echo "  (none set)"

# DIAGNOSTIC: what did merge+olddefconfig change vs the known-good SLOT config?
# The fat kernel hung on the EA1 bringing up the eMMC (mmc0 SDHCI ADMA/IRQ timeout,
# error -110) even though the SLOT kernel — same source, same cmdline — brings it up
# fine. So enabling DRM/SGX cascaded olddefconfig into flipping some storage/DMA/IRQ
# symbol. Dump the full delta (slot -> fat) and spotlight the subsystems that would
# explain an SDHCI ADMA/IRQ failure. normalize "# X is not set" to "X=n" so a flip
# either direction shows.
norm() { sed -E 's/^# (CONFIG_[A-Za-z0-9_]+) is not set/\1=n/; /^#/d; /^$/d' "$1" | sort; }
echo "=== mk-real-kernel: CONFIG DELTA (slot -> fat) ============================"
diff <(norm "$SLOTCFG") <(norm "$SRC/.config") | grep -E '^[<>]' || echo "  (no delta — unexpected)"
echo "=== storage / DMA / IOMMU / IRQ symbols in the FAT config ================="
grep -E 'CONFIG_(MMC|SDHCI|MMC_SDHCI|DMA|DMADEVICES|IOMMU|INTEL_IOMMU|SWIOTLB|PCI_MSI|X86_LOCAL_APIC|X86_IO_APIC|IRQ_REMAP|DMAR)' "$SRC/.config" | sort || true
echo "=========================================================================="
rm -f "$SLOTCFG"

# Sanity: the whole point is these must be built IN (=y), not modules.
for sym in CONFIG_DRM CONFIG_SGX545_CE; do
	if ! grep -q "^$sym=y" "$SRC/.config"; then
		echo "mk-real-kernel: WARNING $sym is not =y in the real kernel (.config says: $(grep "$sym" "$SRC/.config" || echo unset))" >&2
	fi
done

echo "mk-real-kernel: building fat bzImage (DRM/SGX built in) ..."
"${MAKE[@]}" bzImage

BZ="$SRC/arch/x86/boot/bzImage"
[ -f "$BZ" ] || { echo "mk-real-kernel: no bzImage produced at $BZ" >&2; exit 1; }
mkdir -p "$(dirname "$OUTFILE")"
cp -f "$BZ" "$OUTFILE"
echo "mk-real-kernel: real kernel -> $OUTFILE ($(wc -c < "$OUTFILE") bytes, no size limit — kexec'd)"
