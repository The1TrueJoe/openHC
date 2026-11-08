#!/usr/bin/env bash
# HC-250 post-image: zImage + DTB -> legacy uImage, plus the c4test.img script
# stock U-Boot sources off a FAT USB stick when the ID button is held at
# power-on. RAM only; nothing is written to NAND.
#
# $1 = BINARIES_DIR, $2 = board name.
set -euo pipefail
IMAGES="$1"
BOARD="${2:-hc250}"
LOADADDR=0x80008000   # kernel entry; bootm copies the image here
FILEADDR=0x82000000   # where the script fatloads it (stock ${bootload})
MAX=$((0x82000000 - 0x80008000))

MKIMAGE="${HOST_DIR:-}/bin/mkimage"
[ -x "$MKIMAGE" ] || MKIMAGE=mkimage

z="$IMAGES/zImage"
dtb="$IMAGES/c4-omap3-hc250.dtb"
uimg="$IMAGES/openhc-$BOARD-kernel.img"
[ -f "$z" ]   || { echo "post-image: no zImage at $z" >&2; exit 1; }
[ -f "$dtb" ] || { echo "post-image: no DTB at $dtb" >&2; exit 1; }

cat "$z" "$dtb" > "$IMAGES/zImage-dtb"
size=$(stat -c %s "$IMAGES/zImage-dtb")
[ "$size" -lt "$MAX" ] || { echo "post-image: zImage+DTB is $size B, over the $MAX B below $FILEADDR" >&2; exit 1; }
"$MKIMAGE" -A arm -O linux -T kernel -C none -a "$LOADADDR" -e "$LOADADDR" \
    -n "openHC $BOARD" -d "$IMAGES/zImage-dtb" "$uimg" >/dev/null

# U-Boot script: c4sys usbcheck -> usb start -> fatload c4test.img -> source.
# A failed load falls through to the stock boot.
cat > "$IMAGES/c4test.cmd" <<EOF
echo openHC: RAM boot from USB
setenv bootargs console=ttyS0,115200n8 eth=\${ethaddr} ohc.ramboot=1
fatload usb 0:1 $FILEADDR openhc-$BOARD-kernel.img
bootm $FILEADDR
EOF
"$MKIMAGE" -A arm -O linux -T script -C none -n "openHC $BOARD" \
    -d "$IMAGES/c4test.cmd" "$IMAGES/c4test.img" >/dev/null

echo "post-image: $uimg ($size B)"
echo "post-image: $IMAGES/c4test.img"
echo "post-image: copy both to a FAT USB stick, plug it into the rear USB port,"
echo "post-image: power on holding ID for ~2 s, then let go (never 7 s)."
