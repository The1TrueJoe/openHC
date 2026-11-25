#!/bin/sh
# Checks boot-init's two decisions that must never get it wrong: whether p1 is
# a factory-button restore, and whether the gap holds an image worth writing.
#   sh board/ea/common/boot-init/test.sh
set -e
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
P=$PATH; OHC_INIT_FUNCS=1 . "$(dirname "$0")/init"; PATH=$P
fail() { echo "FAIL: $*"; exit 1; }

# p1 roots
mkdir -p "$T/stock/control4" "$T/ohc/control4" "$T/ohc/etc" "$T/inst/control4"
: > "$T/ohc/etc/openhc-release"
: > "$T/inst/.ohc-install-pending"
button_restored "$T/stock" || fail "stock p1 not seen as a button restore"
! button_restored "$T/ohc" || fail "openHC p1 seen as a button restore"
! button_restored "$T/inst" || fail "stock p1 with an install pending seen as a button restore"
! button_restored "$T/empty" || fail "empty p1 seen as a button restore"

# The gap: 16 MiB of anything, then the staged gzip.
ROOTFS_STAGE_MB=1
STAGE_DEV=$T/emmc
gap() { dd if=/dev/zero bs=1M count=1 2>/dev/null; cat "$1"; }
{ dd if=/dev/zero bs=1080 count=1; printf '\123\357'; dd if=/dev/zero bs=1k count=4; } 2>/dev/null | gzip >"$T/ext2.gz"
dd if=/dev/zero bs=1k count=8 2>/dev/null | gzip >"$T/zero.gz"
dd if=/dev/zero bs=512 count=1 2>/dev/null >"$T/wiped"
gap "$T/ext2.gz" >"$STAGE_DEV"; staged_ext2 || fail "staged ext2 image rejected"
gap "$T/zero.gz" >"$STAGE_DEV"; ! staged_ext2 || fail "non-ext2 image accepted"
gap "$T/wiped" >"$STAGE_DEV"; ! staged_ext2 || fail "wiped gap accepted"
echo "boot-init: ok"
