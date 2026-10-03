# SPDX-License-Identifier: GPL-2.0
# openHC: CE5300 audio. Copied into the kernel tree by OHC_KERNEL_MIRROR_HOOK
# together with these sources; sound/soc/Makefile is told to descend here by an
# entry in board/ea/common/drivers/objs.mk.
#
# Literal obj-m, not obj-$(CONFIG_...): there is no Kconfig hunk declaring a symbol
# for these copied-in drivers, so obj-$(CONFIG_...) would silently expand to
# nothing. The whole directory is gated by CONFIG_SND_SOC (=m) deciding whether
# sound/soc is descended into at all; sound/soc/Makefile reaches us via
# `obj-m += ce5300/` (objs.mk), so these build as modules and load from p1 to keep
# the ALSA SoC core out of the bzImage (S45ea-audio). ce5300-ea-board is a module
# too — its initcall becomes module_init and registers the platform device the
# machine driver binds; it calls no snd-soc symbol.
obj-m += ce5300-i2s.o
obj-m += ce5300-ea3.o
obj-m += ce5300-ea-board.o
# EA1 HDMI-audio (the `audio-hdmi` feature). ce5300-hdmi is the platform + cpu-DAI
# skeleton, ce5300-ea1 the machine driver, ce5300-ea1-board the glue that
# registers both platform devices from its module_init. Same obj-m gating as the
# EA3 drivers above: no Kconfig symbol, built when CONFIG_SND_SOC=m descends
# sound/soc. On a board without audio-hdmi (SND_SOC unset) none of these build;
# on EA3 they build but are never loaded (S45ea-audio modprobe is best-effort).
obj-m += ce5300-hdmi.o
obj-m += ce5300-ea1.o
obj-m += ce5300-ea1-board.o
