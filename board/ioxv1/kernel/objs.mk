# Kernel Makefiles that the IO EXTENDER's drivers must be registered in.
#   <Makefile path relative to the kernel tree>|<obj line to append>
# Consumed by OHC_KERNEL_MIRROR_HOOK in board/external.mk. Append-only and
# idempotent: the hook skips any line whose object is already present.
#
# obj-y because these images are all-builtin (CONFIG_MODULES=n). obj-m here
# would expand to nothing and the driver would silently not exist.
#
# Gated by BR2_OHC_IOXV1_KERNEL_DRIVERS so this never reaches another board:
# every line of it drives a Xilinx part wired to seven specific DM355 GPIOs,
# and the compatible it binds exists in exactly one device tree.
drivers/misc/Makefile|obj-y += ohc-iox-fpga.o
