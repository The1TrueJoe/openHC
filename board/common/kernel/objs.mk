# Kernel Makefiles that openHC's SHARED drivers must be registered in.
#   <Makefile path relative to the kernel tree>|<obj line to append>
# Consumed by OHC_KERNEL_MIRROR_HOOK in board/external.mk. Append-only and
# idempotent: the hook skips any line whose object is already present.
#
# obj-y, because these images are deliberately all-builtin (CONFIG_MODULES=n —
# see the rationale at the top of hc800.fragment: self-contained image, no
# depmod, no load ordering, no .ko in the initrd). obj-m here would expand to
# nothing and the driver would silently not exist, which is exactly the trap
# documented in ea-common/kernel/objs.mk.
#
# Built-in does not cost configurability: module_param entries still appear
# under /sys/module/gpio_ohc_iomcu/parameters/, and the ones that describe the
# board are writable there. The init script sets them from board.env and THEN
# attaches the line discipline, which is when the geometry is read.
# NOTE: drivers/Makefile only descends into gpio/ when CONFIG_GPIOLIB=y, so this
# line quietly builds nothing on a board without it. Every openHC board has
# GPIOLIB, but if one ever does not, the symptom is an absent gpiochip and no
# error anywhere — the same silent-drop shape as the obj-m trap above.
#
# CONFIG_RC_CORE, not obj-y, and that is not a style preference. This driver
# registers an rc_dev per IR emitter, so it hard-references rc_allocate_device,
# rc_register_device and ir_raw_event_store. RC_CORE is set by exactly one
# thing — features/iomcu/linux.fragment — which is selected by exactly the
# boards that HAVE an IO microcontroller (hc800, ea-common). obj-y therefore
# built a driver for absent hardware on the IO Extender and the CA-1, and on
# those two the link failed with a screen of undefined references to rc-core.
#
# The condition is the driver's real dependency rather than a
# board-has-an-MCU flag because there is nowhere to define such a flag: the
# mirror hook appends lines to kernel Makefiles and cannot add a Kconfig
# symbol. If ioxv1 later enables RC_CORE for its own FPGA IR block this line
# starts building again — harmless (it links, and probes nothing without a
# line discipline attached), but the day that happens is the day to give this
# a symbol of its own.
drivers/gpio/Makefile|obj-$(CONFIG_RC_CORE) += gpio-ohc-iomcu.o
