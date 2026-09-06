#!/usr/bin/env bash
# Build the tiny boot-initramfs (boot-init.cpio.gz) that the EA autoscript loads.
#
# It carries only what board/ea-common/boot-init/init needs: busybox, its
# dynamic loader + libc, the /init itself, and the applet symlinks the script
# calls. A few hundred KB to ~2 MB, versus the ~15-20 MB full rootfs — so a
# normal boot unpacks almost nothing before switch_root frees it.
#
#   mk-boot-init.sh <TARGET_DIR> <OUT_cpio_gz> <init_script>
#
# TARGET_DIR is the assembled full rootfs (for busybox + libs); we copy out of
# it rather than rebuild, so the boot-init and the rootfs always share one
# busybox/libc ABI.
set -euo pipefail
TARGET="$1"; OUT="$2"; INIT="$3"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK"/{bin,sbin,dev,proc,sys,mnt,lib}

cp -a "$TARGET/bin/busybox" "$WORK/bin/busybox"
install -m755 "$INIT" "$WORK/init"

# Dynamic loader (the ELF interpreter busybox names) + libc + the handful of
# libraries busybox commonly pulls in. The loader is NOT optional — without it
# the kernel cannot exec busybox and /init never runs (the box hangs). Copy
# whatever of these exists in the target; missing optional ones are harmless.
# We glob the whole set rather than parse the interpreter path, because that
# parse was fragile in-container and silently dropped ld-linux.so.2.
mkdir -p "$WORK/lib"
for pat in 'ld-linux*.so*' 'ld-musl*.so*' 'ld-*.so*' \
           'libc.so*' 'libm.so*' 'libresolv.so*' 'libcrypt.so*' \
           'libpthread.so*' 'libdl.so*' 'librt.so*'; do
    for f in "$TARGET/lib/"$pat; do
        [ -e "$f" ] && cp -aL "$f" "$WORK/lib/" 2>/dev/null || true
    done
done
# Sanity: refuse to ship a boot-init busybox cannot start.
if ! ls "$WORK"/lib/ld-*.so* >/dev/null 2>&1; then
    echo "mk-boot-init: ERROR no dynamic loader (ld-*.so) found in $TARGET/lib" >&2
    exit 1
fi

# Applet symlinks the /init calls (busybox resolves the rest by argv[0]).
for a in sh mount umount mkdir dd gunzip gzip sync switch_root cat ls; do
    ln -sf /bin/busybox "$WORK/bin/$a"
done
ln -sf /bin/busybox "$WORK/sbin/switch_root"

( cd "$WORK" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$OUT"
echo "mk-boot-init: $OUT ($(wc -c < "$OUT") bytes)"
