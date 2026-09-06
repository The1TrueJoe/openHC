# SPDX-License-Identifier: GPL-2.0
# openHC: CE5300 audio. Copied into the kernel tree by OHC_KERNEL_DRIVERS_HOOK
# together with these sources; sound/soc/Makefile is told to descend here by an
# entry in board/ea-common/drivers/objs.mk.
#
# Plain obj-y, like openHC's other copied-in drivers: there is no Kconfig hunk
# declaring a symbol for these, so obj-$(CONFIG_...) would silently expand to
# nothing. The whole directory is already gated by CONFIG_SND_SOC, which is what
# decides whether sound/soc is descended into at all.
obj-y += ce5300-i2s.o
obj-y += ce5300-ea3.o
obj-y += ce5300-ea-board.o
