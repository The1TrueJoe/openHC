# Kernel Makefiles that openHC's SHARED drivers must be registered in.
#   <Makefile path relative to the kernel tree>|<obj line to append>
# Consumed by OHC_COMMON_KERNEL_HOOK in board/external.mk. Append-only and
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
drivers/gpio/Makefile|obj-y += gpio-ohc-iomcu.o
