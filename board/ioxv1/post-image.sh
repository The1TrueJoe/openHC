#!/usr/bin/env bash
# Buildroot post-image hook for the DM355 IO Extender V1.
#
# The stock Control4 U-Boot (IOX v1.2.0) passes ATAGS and boots a legacy uImage;
# it can't hand over a DTB. So we append the board DTB to the zImage (the kernel
# finds it via CONFIG_ARM_APPENDED_DTB) and wrap the result as a uImage loaded at
# 0x80008000. Buildroot's own APPENDED_UIMAGE mangles the vendored dts path, so
# we do it here by hand with host mkimage (BR2_PACKAGE_HOST_UBOOT_TOOLS).
#
# $1 = BINARIES_DIR (output/images). $2 = board name (BR2_ROOTFS_POST_SCRIPT_ARGS).
# The initramfs rootfs is already embedded in the zImage (BR2_TARGET_ROOTFS_INITRAMFS).
set -euo pipefail
IMAGES="$1"
BOARD="${2:-ioxv1}"
LOADADDR="0x80008000"

z="$IMAGES/zImage"
dtb="$IMAGES/dm355-hammer.dtb"
out="$IMAGES/openhc-$BOARD-kernel.img"

[ -f "$z" ]   || { echo "post-image: no zImage at $z" >&2; exit 1; }
[ -f "$dtb" ] || { echo "post-image: no dtb at $dtb (INTREE_DTS_NAME built?)" >&2; exit 1; }

cat "$z" "$dtb" > "$IMAGES/zImage-dtb"
mkimage -A arm -O linux -T kernel -C none -a "$LOADADDR" -e "$LOADADDR" \
        -n "openHC $BOARD (DM355 7.1.8)" \
        -d "$IMAGES/zImage-dtb" "$out"

echo "post-image: netboot uImage ready -> $out"
echo "post-image:   serve it to U-Boot and 'run tst' (TFTP into RAM, no flash)."
[ -f "$IMAGES/rootfs.jffs2" ] && \
    echo "post-image: flash rootfs        -> $IMAGES/rootfs.jffs2 (nandwrite to a spare bank)"
exit 0
