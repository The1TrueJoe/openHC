include $(sort $(wildcard $(BR2_EXTERNAL_OPENHC_PATH)/package/*/*.mk))

# ── EA3: wire the BCM53125 board glue into the kernel build ─────────────────
#
# board/ea3/patches/linux/0003-* drops drivers/spi/spi-ea3-board.c into the
# tree, but a new .c file is not built until something in a Makefile references
# it. Adding that line by patch is what broke the first EA3 build:
#
#     patching file drivers/spi/Makefile
#     Hunk #1 FAILED at 1.
#
# The hunk's context was written from a guess at that file's opening lines. A
# post-patch hook has no context to drift against, so it cannot fail that way.
#
# Self-guarding on purpose: it fires only when the EA3 patch actually put the
# file there, so an EA1 build (same external tree, no such patch) is a no-op —
# no board conditional needed. The grep makes it idempotent across re-patching.
define OHC_EA3_SPI_BOARD_HOOK
	if [ -f $(LINUX_DIR)/drivers/spi/spi-ea3-board.c ] && \
	   ! grep -q 'spi-ea3-board\.o' $(LINUX_DIR)/drivers/spi/Makefile; then \
		echo '' >> $(LINUX_DIR)/drivers/spi/Makefile; \
		echo '# openHC: Control4 EA3 board glue (BCM53125 on SPI bus 0 CS 1).' \
			>> $(LINUX_DIR)/drivers/spi/Makefile; \
		echo 'obj-y += spi-ea3-board.o' >> $(LINUX_DIR)/drivers/spi/Makefile; \
		echo 'openHC: appended spi-ea3-board.o to drivers/spi/Makefile'; \
	fi
endef
LINUX_POST_PATCH_HOOKS += OHC_EA3_SPI_BOARD_HOOK
