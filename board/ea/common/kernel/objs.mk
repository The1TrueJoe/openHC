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
# Board DESCRIPTION is not here: the switch, codec, LEDs, reset lines, LM75 and
# framebuffer are nodes in arch/x86/boot/dts/ (linked in by BUILTIN_DTB). Only
# drivers for the SoC's own PCI functions belong in this table.
drivers/gpio/Makefile|obj-y += gpio-intelce.o
drivers/pwm/Makefile|obj-y += pwm-ce5300.o
# Boot SPI-NOR controller: the boot flash is NOT on the pxa2xx SSP (8086:2e6a,
# the general-purpose SPI the switch rides). It is on a DEDICATED block, PCI
# 01:17.0 "FLASH memory [0501]" 8086:08a0, which has no mainline driver. This is
# a spi-mem port of Intel's out-of-tree ce5xx_spi_flash; it registers the master
# with its DTB node, whose flash@0 child (ce5300.dtsi) in-tree spi-nor binds, so
# openHC gets /dev/mtd0 -- what a software stock-restore and an in-place
# autoscript rewrite need. Unconditional; the flash is always present.
drivers/spi/Makefile|obj-y += spi-ea-ce5xx.o

# --- audio (ASoC lives outside drivers/; the hook mirrors sound/ too) ---
# The codec goes in the existing codecs/ dir. The CE5300 I2S platform driver gets
# its own directory, so sound/soc/Makefile is told to descend into it.
# obj-m, not obj-y: the SoC core and these drivers are built as modules so the
# ea3 bzImage stays inside CEFDK's bootlinux window — udev loads them from p1 off
# their devices' modaliases (same move as the switch's b53). See
# features/audio-dsp/linux.fragment.
sound/soc/codecs/Makefile|obj-m += adau1451-c4.o
sound/soc/Makefile|obj-m += ce5300/
# HDMI audio is the HDMI transmitter's own DMA, not the I2S block, and is a plain
# ALSA card rather than ASoC, so it sits in sound/x86 (descended whenever
# CONFIG_SND is set). Every EA has HDMI; features/audio-hdmi turns ALSA on.
sound/x86/Makefile|obj-m += ce5300-hdmi.o

# --- GPU: PowerVR SGX545 (the sgx545ce submodule mirrored in beside it) ---
# Unlike the drivers above (openHC's own single .c files, obj-y), this is a large
# vendored DDK with its own Kbuild + Kconfig, so it is gated by a REAL symbol and
# built as a MODULE (=m, set in features/sgx/linux.fragment) to keep ~1.25 MiB of
# DRM core out of the bzImage. The hook copies its Kbuild/Kconfig + src/ tree in
# (find now matches Kbuild/Kconfig) and wires both lines below.
drivers/gpu/drm/Kconfig|source "drivers/gpu/drm/sgx545ce/Kconfig"
drivers/gpu/drm/Makefile|obj-$(CONFIG_SGX545_CE) += sgx545ce/
