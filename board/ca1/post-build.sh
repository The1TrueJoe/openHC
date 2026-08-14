#!/usr/bin/env bash
# Buildroot post-build hook for the Control4 CA-1.
#
# One job: install rtlwifi/rtl8723bs_nic.bin, the Wi-Fi firmware for the
# RTL8723BS on usdhc1.
#
# Why this is not just a Buildroot symbol: drivers/staging/rtl8723bs does
# request_firmware("rtlwifi/rtl8723bs_nic.bin") in rtl8723b_hal_init.c and has
# no embedded firmware array, but Buildroot's BR2_PACKAGE_LINUX_FIRMWARE_RTL_87XX
# file list omits that exact name (it ships rtl8723bs_bt.bin and
# rtl8723bu_nic.bin). The file does exist in the linux-firmware tarball that
# package already downloaded and extracted, so we copy it out of $BUILD_DIR
# instead of vendoring a binary into this repo or adding a second download.
#
# Non-fatal by design. Wi-Fi is not the bring-up reachability path — eth0 is —
# so a missing firmware file must not break the image. It warns instead.
#
# $1 = TARGET_DIR. $BUILD_DIR is exported by Buildroot.
set -euo pipefail
TARGET="$1"
FW_REL="rtlwifi/rtl8723bs_nic.bin"

dest="$TARGET/lib/firmware/$FW_REL"
[ -f "$dest" ] && exit 0   # already installed (symbol list changed upstream?)

src=""
for d in "${BUILD_DIR:-}"/linux-firmware-*; do
    [ -f "$d/$FW_REL" ] && { src="$d/$FW_REL"; break; }
done

if [ -n "$src" ]; then
    install -D -m 0644 "$src" "$dest"
    echo "post-build: installed $FW_REL (rtl8723bs Wi-Fi)"
else
    echo "post-build: WARNING $FW_REL not found under \$BUILD_DIR/linux-firmware-*" >&2
    echo "post-build:   Wi-Fi will probe and then fail to load firmware." >&2
    echo "post-build:   Ethernet is unaffected; bring-up does not depend on this." >&2
fi
