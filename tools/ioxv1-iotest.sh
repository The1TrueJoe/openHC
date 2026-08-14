#!/bin/sh
# openHC IO Extender V1 (DM355) front-panel / relay test harness.
#
# Run this ON THE BOARD (over SSH) with the front panel visible and your ear
# near the relays. It drives each LED/relay GPIO both HIGH and LOW so we can
# see, in one pass, (a) which pins actually reach the pad and (b) the correct
# polarity -- the two things we couldn't determine remotely while you slept.
#
#   ./ioxv1-iotest.sh          # test with the pinmux as U-Boot left it
#   ./ioxv1-iotest.sh nomux    # first clear PINMUX3 (video block), then test
#   ./ioxv1-iotest.sh restore  # put every tested pin back to input (safe)
#
# Background (why both polarities): the board file says relays/LEDs are
# active-high (drive HIGH = on), but driving HIGH produced nothing, and DM355
# GPIO IN_DATA does NOT read back outputs so we can't verify in software. If a
# pin responds to LOW instead, it's wired active-low; if a pin responds only
# after `nomux`, it was muxed to the video block. GPIO101 (dm9000 reset) is a
# known-good GPIO output, so the GPIO block itself works.
#
# DM355 GPIO regs (base 0x01c67000): bank for gpio 64-95 is at +0x60.
DIRR=0x01c67060; OUTR=0x01c67064; SETR=0x01c67068; CLRR=0x01c6706c
PINMUX3=0x01c4000c

d() { devmem "$@"; }
mk_output() { cur=$(d $DIRR 32); d $DIRR 32 $(printf 0x%x $(( cur & ~$1 ))); }
release()   { cur=$(d $DIRR 32); d $DIRR 32 $(printf 0x%x $(( cur | $1 ))); }
high() { mk_output $1; d $SETR 32 $1; }
low()  { mk_output $1; d $CLRR 32 $1; }

test_pin() {   # $1=gpio  $2=bitmask  $3=label
	printf '  %-9s (gpio %s)  HIGH ' "$3" "$1"; high $2; sleep 2
	printf '/ LOW '; low $2; sleep 2
	release $2; echo '/ released'
}

case "${1:-}" in
	nomux)
		echo "Clearing PINMUX3 (was $(d $PINMUX3 32)) -> 0x11000000 (disable video block)"
		d $PINMUX3 32 0x11000000 ;;
	restore)
		for b in 0x800 0x1000 0x01000000 0x02000000 0x04000000 0x08000000 \
		         0x10000000 0x20000000 0x40000000 0x80000000; do release $b; done
		echo "all tested pins back to input"; exit 0 ;;
esac

echo "=== LEDs (watch the front panel) ==="
test_pin 75 0x800  "data"
test_pin 76 0x1000 "link"
echo "=== RELAYS (listen for clicks) ==="
n=0
for b in 0x01000000 0x02000000 0x04000000 0x08000000 \
         0x10000000 0x20000000 0x40000000 0x80000000; do
	n=$((n + 1)); test_pin $((87 + n)) $b "relay$n"
done
echo "=== done. Note which lit/clicked and on HIGH or LOW. 'restore' resets. ==="
