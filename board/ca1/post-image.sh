#!/usr/bin/env bash
# Buildroot post-image hook for the Control4 CA-1.
#
# Nothing to wrap here: the EA family needs a CEFDK container and the DM355
# needs a legacy uImage, but the CA-1's stock U-Boot uses `bootz` on a raw
# zImage. So this step publishes the zImage under the openHC image name and
# builds the one artifact that makes bring-up cheap on this board — a boot.scr.
#
# Why boot.scr: stock `bootcmd` tries `fatload mmc 1:1 ${loadaddr} boot.scr;
# source` BEFORE it falls through to loading the stock zImage. There is no
# boot.scr on the unit, so dropping one onto the eMMC vfat partition takes over
# the boot path with no serial console, no bootloader reflash and no fuse
# changes — and deleting that one file restores stock. See docs/ca1-recon.md.
#
# $1 = BINARIES_DIR (output/images). $2 = board name (BR2_ROOTFS_POST_SCRIPT_ARGS).
set -euo pipefail
IMAGES="$1"
BOARD="${2:-ca1}"

DTB="c4-imx6sl-${BOARD}.dtb"
KERNEL_FILE="openhc-${BOARD}-zImage"

# Vendor loadaddr, which we know U-Boot is happy with. fdt_high=ffffffff in
# the stock environment means U-Boot leaves the DTB where it is put.
#
# FDTADDR must clear the DECOMPRESSED zImage (kernel + embedded initramfs),
# which unpacks from ~LOADADDR, but must ALSO stay low. Both boundaries were
# found the hard way on hardware (serial console):
#
#  * 0x83000000 (40 MB past LOADADDR) boots a lean ~17 MB zImage cleanly, all
#    the way to /init — this is the value the first working boot used.
#  * A 32 MB zImage (Node.js embedded in the initramfs) decompressed far enough
#    to clobber the DTB at 0x83000000 and hung at "Starting kernel" with no
#    output. The fix for THAT is to keep the initramfs lean (Node/heavy services
#    belong on the eMMC ext4 rootfs, not the RAM image) — NOT to move the DTB up.
#  * 0x90000000 was tried as a "just move it higher" fix and is WORSE: the DTB
#    reads fine there (the memory node parses) but the kernel then hangs after
#    "crng init done", before "Freeing unused kernel memory" — it never reaches
#    /init. 0x83000000 with the same lean image boots to a login prompt. So high
#    is not safe here; keep the DTB at 0x83000000 and the image lean.
LOADADDR=0x80800000
FDTADDR=0x83000000

zimg="$IMAGES/zImage"
out="$IMAGES/openhc-$BOARD-kernel.img"
[ -f "$zimg" ] || { echo "post-image: no zImage at $zimg (kernel build incomplete?)" >&2; exit 1; }
[ -f "$IMAGES/$DTB" ] || { echo "post-image: no $DTB at $IMAGES (DTS build failed?)" >&2; exit 1; }

cp -f "$zimg" "$out"

# --- the boot script ---------------------------------------------------------
# Filenames are openHC-specific on purpose: this must not collide with the
# stock zImage / c4-imx6sl-0-4.dtb already on that partition, so a failed boot
# can be recovered by deleting boot.scr alone.
cat > "$IMAGES/boot.cmd" <<EOF
# openHC $BOARD — sourced by stock Control4 U-Boot 2014.04 from mmc 1:1.
# Delete boot.scr from that partition to fall back to the stock boot path.
echo openHC: booting $BOARD from mmc 1:1
setenv bootargs console=ttymxc0,115200 root=/dev/mmcblk1p2 rootwait rw initcall_debug ignore_loglevel
fatload mmc 1:1 $LOADADDR $KERNEL_FILE
fatload mmc 1:1 $FDTADDR $DTB
bootz $LOADADDR - $FDTADDR

# NOTE the "initcall_debug ignore_loglevel" on the cmdline. It is NOT here for
# debugging — it is a working WORKAROUND, needed to boot. Confirmed on hardware
# 2026-08-18: with a normal fast boot, something in the console/printk path on
# this SoC melts down early — a single message ("random: crng init done" with the
# RNGB on, or the last serial-port registration with it off) repeats at the full
# serial bitrate and the boot never gets past it. Serialising the boot with
# initcall_debug (each initcall bracketed by a print) slows the message rate
# enough that it doesn't, and the box boots cleanly all the way to the login
# prompt + the web UI. TODO: root-cause the flood (likely a printk re-entrancy /
# console lock issue) and drop this; until then it is load-bearing.
EOF

MKIMAGE="${HOST_DIR:-}/bin/mkimage"
if [ -x "$MKIMAGE" ]; then
    "$MKIMAGE" -A arm -O linux -T script -C none -n "openHC $BOARD" \
        -d "$IMAGES/boot.cmd" "$IMAGES/boot.scr" >/dev/null
    cp -f "$zimg" "$IMAGES/$KERNEL_FILE"
    echo "post-image: boot script         -> $IMAGES/boot.scr"
else
    echo "post-image: WARNING mkimage not found at $MKIMAGE — boot.scr not built." >&2
    echo "post-image:   enable BR2_PACKAGE_HOST_UBOOT_TOOLS, or build it by hand:" >&2
    echo "post-image:   mkimage -A arm -O linux -T script -C none -d boot.cmd boot.scr" >&2
fi

echo "post-image: netboot/boot kernel -> $out"
echo "post-image: device tree         -> $IMAGES/$DTB"
echo "post-image:"
echo "post-image: install (non-destructive — writes only to the vfat partition):"
echo "post-image:   mount /dev/mmcblk1p1 /mnt"
echo "post-image:   cp $KERNEL_FILE $DTB boot.scr /mnt/"
echo "post-image:   umount /mnt && reboot"
echo "post-image: recover: delete boot.scr from that partition."

if [ -f "$IMAGES/rootfs.ext4" ]; then
    echo "post-image: eMMC rootfs         -> $IMAGES/rootfs.ext4 (only once the kernel is trusted)"
fi
