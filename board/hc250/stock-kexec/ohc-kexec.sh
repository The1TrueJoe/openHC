#!/bin/sh
# ohc-kexec.sh <image|http-url> [dry] [-- extra cmdline]
# Boot an openHC uImage/zImage from the stock HC-250 OS (2.6.32, BusyBox 1.15)
# without U-Boot. RAM only: a reboot or power cycle comes back up in stock.
# "dry" loads and plans everything, prints it to dmesg and stays in stock.
set -e
img=$1; shift || true
mode=1
[ "$1" = dry ] && { mode=dry; shift; }
[ "$1" = -- ] && shift
[ -n "$img" ] || { echo "usage: $0 <image|http-url> [dry] [-- extra cmdline]" >&2; exit 2; }

ko=${KO:-$(dirname "$0")/ohc_kexec.ko}
p=/sys/module/ohc_kexec/parameters

kln=$(grep ' T kallsyms_lookup_name$' /proc/kallsyms | cut -d' ' -f1)
[ -n "$kln" ] || { echo "no kallsyms_lookup_name in /proc/kallsyms" >&2; exit 1; }
mac=$(sed -n 's/.*eth=\([0-9a-fA-F:]*\).*/\1/p' /proc/cmdline)

# max_mb: room for the image at load_addr (the audio build is ~47 MB).
[ -d $p ] || insmod "$ko" kallsyms=0x$kln max_mb=${MAX_MB:-96}
dev=/dev/ohc-kexec
if [ ! -c $dev ]; then
    mm=$(cat /sys/class/misc/ohc-kexec/dev)
    mknod $dev c ${mm%:*} ${mm#*:}
fi

# 720p forced on: the DPI bus (component, splash) runs with no HDMI monitor.
echo "console=ttyS0,115200n8 eth=$mac video=HDMI-A-1:1280x720@60e ohc.ramboot=1 $*" > $p/cmdline
case "$img" in
    http://*|ftp://*) wget -O - "$img" > $dev ;;
    *) cat "$img" > $dev ;;
esac
dmesg | grep ohc-kexec | tail -1
[ $mode = dry ] || { echo "ohc-kexec: jumping; the console moves to the new kernel"; sync; }
echo $mode > $p/go
dmesg | grep ohc-kexec | tail -40
