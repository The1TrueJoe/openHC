#!/bin/sh
# ohc-boot-env.sh install | uninstall | status | stock-io   (runs on STOCK)
#
# The env half of the HC-250 SD install; openHC's ohc-sd-install does the SD and
# EEPROM half first. Stock's fw_setenv writes the NAND with Control4's own ECC
# layout, which mainline's driver does not match, so this half runs on stock.
#
# install  bootohc/bootohcm/ohc_bootargs, restoresystem += "setenv bootmode nand;
#          saveenv" (factory restore then returns U-Boot to stock), and last
#          bootmode=ohc; ohc_arm=1 makes the next boot zero the try count.
#          bootohc counts tries in EEPROM byte 6 and boots stock
#          (bootnand) at 3 or when the SD has no image; EEPROM byte 7 = 'R' runs
#          the factory restore (openHC's `restore stock`).
# uninstall bootmode=nand and stock's restoresystem back; the ohc variables go.
# stock-io  a tar of this stock's IO firmware (/control4/firmware/io) on stdout,
#           for openHC's `ohc-sd-install stock-io` to keep on the SD: `restore
#           stock` writes it back to the IO microcontroller (openHC cannot read
#           the stock rootfs on the NAND).
set -e

STOCK_RESTORE='run restorekern; run restorerootfs; run cleanboot'
OHC_RESTORE="$STOCK_RESTORE; setenv bootmode nand; saveenv"
BOOTARGS='console=ttyS0,115200n8 video=HDMI-A-1:1280x720@60e panic=10 ohc.boot=sd'
BOOTOHCM='setenv bootargs mem=${memsize} eth=${ethaddr} ${ohc_bootargs}; bootm ${bootload}'
# U-Boot 2011.03: i2c bus 0 = I2C1 (the 24C02 at 0x50); generic MMC, no "mmc init".
# EEPROM byte 7 = 0x52 ('R', set by openHC's `restore stock`): clear it, zero the
# c4sys counters as the button does, run Control4's factory restore (which, with
# the install's hook, also sets bootmode=nand), and boot the restored stock.
BOOTOHC='i2c dev 0; i2c read 50 7 1 8a000000;'
BOOTOHC="$BOOTOHC"' if itest.b *8a000000 -eq 52; then i2c mw 50 7 ff; i2c mw 50 0 0 4; echo openHC: factory restore requested; run restoresystem; run bootnand; fi;'
# ohc_arm=1 (set by install): zero the try count once, so a stock that cannot
# reach the EEPROM still points the box back at openHC after a fallback.
BOOTOHC="$BOOTOHC"' if itest.s "x${ohc_arm}" == x1; then i2c mw 50 6 0; setenv ohc_arm; saveenv; fi;'
BOOTOHC="$BOOTOHC"' i2c read 50 6 1 8a000000;'
BOOTOHC="$BOOTOHC"' if itest.b *8a000000 -ge 3; then echo openHC: 3 failed boots, booting stock; run bootnand; fi;'
BOOTOHC="$BOOTOHC"' if itest.b *8a000000 -eq 2; then i2c mw 50 6 3; elif itest.b *8a000000 -eq 1; then i2c mw 50 6 2; else i2c mw 50 6 1; fi;'
BOOTOHC="$BOOTOHC"' if mmc rescan 0 && fatload mmc 0:3 ${bootload} openhc.img; then run bootohcm;'
BOOTOHC="$BOOTOHC"' elif mmc rescan 1 && fatload mmc 1:3 ${bootload} openhc.img; then run bootohcm; fi; run bootnand'

die() { echo "ohc-boot-env: $*" >&2; exit 1; }
# Control4's fw_printenv prints CRC lines before name=value.
getenv() { fw_printenv "$1" 2>/dev/null | sed -n "s/^$1=//p"; }
setv() {
    fw_setenv "$1" "$2"
    [ "$(getenv "$1")" = "$2" ] || die "$1 did not stick"
}

check() {
    case "$(getenv bootcmd)" in *'run boot${bootmode}'*) ;; *) die "bootcmd is not stock's (run boot\${bootmode})" ;; esac
    rs=$(getenv restoresystem)
    [ "$rs" = "$STOCK_RESTORE" ] || [ "$rs" = "$OHC_RESTORE" ] || die "restoresystem is not stock's: $rs"
}

case "$1" in
    install)
        check
        setv ohc_bootargs "$BOOTARGS"
        setv bootohcm "$BOOTOHCM"
        setv bootohc "$BOOTOHC"
        setv restoresystem "$OHC_RESTORE"
        setv ohc_arm 1
        setv bootmode ohc
        echo "installed: next boot is openHC from the SD"
        ;;
    uninstall)
        check
        setv bootmode nand
        setv restoresystem "$STOCK_RESTORE"
        for v in bootohc bootohcm ohc_bootargs ohc_arm; do fw_setenv "$v"; done
        echo "uninstalled: U-Boot boots stock from NAND"
        ;;
    stock-io)
        [ -d /control4/firmware/io ] || die "no /control4/firmware/io on this stock"
        tar -C / -cf - control4/firmware/io
        ;;
    status)
        echo "bootmode=$(getenv bootmode)"
        echo "restoresystem=$(getenv restoresystem)"
        echo "bootohc=$(getenv bootohc)"
        ;;
    *) echo "usage: $0 install | uninstall | status | stock-io" >&2; exit 2 ;;
esac
