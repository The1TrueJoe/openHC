# Kernel Makefiles that openHC's own drivers must be registered in.
#   <Makefile path relative to the kernel tree>|<obj line to append>
# Consumed by OHC_KERNEL_MIRROR_HOOK in board/external.mk. Append-only and
# idempotent: the hook skips any line whose object is already present.
#
# These are plain `obj-y`, not `obj-$(CONFIG_FOO)`, on purpose. A CONFIG symbol
# only exists if something declares it in a Kconfig file, and these drivers are
# copied in rather than patched in — there is no Kconfig hunk to declare one.
# Writing obj-$(CONFIG_PWM_CE5300) here would expand to nothing and the driver
# would silently not build, which is exactly how CONFIG_PWM_CE5300=y sat in
# common.fragment doing nothing. Gate a driver by putting it behind a feature's
# .fragment + a real upstream symbol, not by inventing one.
#
# ce5300-fb is the exception: FB_SIMPLE is a real upstream symbol.
drivers/video/fbdev/Makefile|obj-$(CONFIG_FB_SIMPLE) += ce5300-fb.o
drivers/gpio/Makefile|obj-y += gpio-intelce.o
# Board GPIO init: names the vendor lines and releases the resets. Must build
# alongside gpio-intelce, not instead of it -- it requests lines from that chip.
drivers/gpio/Makefile|obj-y += gpio-ea-board.o
drivers/pwm/Makefile|obj-y += pwm-ce5300.o
drivers/leds/Makefile|obj-y += leds-ea-board.o
drivers/spi/Makefile|obj-y += spi-ea-b53-board.o
# NOTE on the boot SPI-NOR / an /dev/mtd0: the boot flash is NOT on this pxa2xx
# SSP (8086:2e6a) -- that is the general-purpose SPI the switch rides. It is on a
# DEDICATED controller, PCI 01:17.0 "FLASH memory [0501]" 8086:08a0, which has no
# mainline driver (Control4 drove it with a bespoke ce5xx_spi_flash + nmyx25).
# An earlier spi-ea-flash-board.c that hung spi-nor off the SSP only produced
# "SPI transfer failed: -5" because nothing answers on that CS -- removed. Giving
# openHC an MTD means porting a driver for 08a0; it is a separate effort and is
# NOT required for booting the full kernel, which the S00kexec bootstrap already
# does without touching flash.

# --- audio (ASoC lives outside drivers/; the hook mirrors sound/ too) ---
# The codec goes in the existing codecs/ dir. The platform + machine drivers get
# their own directory, so sound/soc/Makefile has to be told to descend into it.
sound/soc/codecs/Makefile|obj-y += adau1451-c4.o
sound/soc/Makefile|obj-y += ce5300/
