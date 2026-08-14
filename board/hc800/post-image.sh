#!/usr/bin/env bash
# Buildroot post-image hook for the Control4 HC-800.
#
# Nothing to wrap: the EA family needs a CEFDK container and the DM355 needs a
# legacy uImage, but this board is a PC with GRUB 0.97 on it, and GRUB loads a
# bare bzImage plus a plain cpio.gz initrd. So this step publishes both under
# openHC names and writes the one artifact that makes the install cheap — the
# menu.lst stanza to paste in.
#
# Why a third menu entry rather than replacing anything: sda1's menu.lst already
# has two entries (factory restore at (hd0,1), the vendor image at (hd0,2)) and
# both stay byte-identical. Only `default` moves. Recovery is one digit, from a
# serial console or over SSH from either image. See docs/hc800-recon.md.
#
# $1 = BINARIES_DIR (output/images). $2 = board name (BR2_ROOTFS_POST_SCRIPT_ARGS).
set -euo pipefail
IMAGES="$1"
BOARD="${2:-hc800}"

# GRUB partition numbering is 0-based, the kernel's is 1-based: (hd0,2) is
# /dev/sda3, the ext3 kernel-only partition. GRUB 0.97 cannot read the ext4
# root on sda4, which is exactly why Control4 put the kernel there too.
GRUB_ROOT="(hd0,2)"
KERNEL_NAME="openhc-bzImage"
INITRD_NAME="openhc-initrd.gz"

bzimage="$IMAGES/bzImage"
cpio="$IMAGES/rootfs.cpio.gz"
out="$IMAGES/openhc-$BOARD-kernel.img"

[ -f "$bzimage" ] || { echo "post-image: no bzImage at $bzimage (kernel build incomplete?)" >&2; exit 1; }
[ -f "$cpio" ]    || { echo "post-image: no rootfs.cpio.gz at $cpio (BR2_TARGET_ROOTFS_CPIO_GZIP off?)" >&2; exit 1; }

cp -f "$bzimage" "$out"
cp -f "$bzimage" "$IMAGES/$KERNEL_NAME"
cp -f "$cpio"    "$IMAGES/$INITRD_NAME"

# --- the menu.lst stanza ----------------------------------------------------
# No root= : the initrd IS the rootfs (the kernel unpacks a cpio.gz initrd as
# initramfs and runs /init from it), so there is nothing to mount and nothing
# on disk to depend on. console= matches GRUB's own `terminal serial` line and
# the getty in this image.
#
# Formatting matches the vendor's stanzas exactly — tab-separated keywords, a
# blank line before the title, a trailing `boot`. GRUB does not need the `boot`
# line, but this file gets appended to a config a human will read next to two
# entries that have one.
cat > "$IMAGES/menu.lst.openhc" <<EOF

title		openHC $BOARD (kernel-up)
root		$GRUB_ROOT
kernel		/boot/$KERNEL_NAME console=ttyS0,115200
initrd		/boot/$INITRD_NAME
boot
EOF

# `wc -c` rather than `stat -c%s`: Buildroot runs this on Linux, but the script
# is also useful to run by hand on a Mac, where BSD stat has no -c.
ksize=$(( $(wc -c < "$out") / 1024 ))
isize=$(( $(wc -c < "$IMAGES/$INITRD_NAME") / 1024 ))

echo "post-image: kernel   -> $out (${ksize} KB)"
echo "post-image: initrd   -> $IMAGES/$INITRD_NAME (${isize} KB)"
echo "post-image: menu.lst -> $IMAGES/menu.lst.openhc"
echo "post-image:"
echo "post-image: install — non-destructive, writes only sda3:/boot and sda1's menu.lst."
echo "post-image: From this host, with the unit running its stock image."
echo "post-image:"
echo "post-image: NOTE: plain 'scp' does NOT work against these units — modern"
echo "post-image: OpenSSH scp speaks SFTP and the vendor dropbear has no"
echo "post-image: sftp-server ('subsystem request failed'). Pipe through ssh, or"
echo "post-image: use 'scp -O' to force the legacy protocol. Both verified."
echo "post-image:"
echo "post-image:   # 1. kernel + initrd onto the ext3 kernel partition"
echo "post-image:   tools/ssh <ip> 'mkdir -p /mnt/k && mount /dev/sda3 /mnt/k'"
echo "post-image:   cat $KERNEL_NAME | tools/ssh <ip> 'cat > /mnt/k/boot/$KERNEL_NAME'"
echo "post-image:   cat $INITRD_NAME | tools/ssh <ip> 'cat > /mnt/k/boot/$INITRD_NAME'"
echo "post-image:   tools/ssh <ip> 'umount /mnt/k'"
echo "post-image:"
echo "post-image:   # 2. append our stanza to sda1's menu.lst as a THIRD entry and"
echo "post-image:   #    point default at it (entries are 0-based, so ours is 2)."
echo "post-image:   tools/ssh <ip> 'mkdir -p /mnt/g && mount /dev/sda1 /mnt/g &&"
echo "post-image:                   cp /mnt/g/boot/grub/menu.lst /mnt/g/boot/grub/menu.lst.stock'"
echo "post-image:   cat menu.lst.openhc | tools/ssh <ip> 'cat >> /mnt/g/boot/grub/menu.lst'"
echo "post-image:   tools/ssh <ip> 'sed -i \"s/^default.*/default\t\t2/\" /mnt/g/boot/grub/menu.lst &&"
echo "post-image:                   umount /mnt/g && reboot'"
echo "post-image:"
echo "post-image: recover: set default back to 1 (or restore menu.lst.stock). The"
echo "post-image: vendor root on sda4 and the factory-restore image on sda2 are"
echo "post-image: never written, so the stock image boots again untouched."
echo "post-image: A serial console on ttyS0 @115200 sees GRUB itself if that fails."
