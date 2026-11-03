################################################################################
#
# io-mcu — openHC's IO microcontroller firmware
#
# Built from this folder by its own Makefile with host-arm-gnu-toolchain; the
# protocol header comes from the driver (board/common/kernel). See README.md.
#
################################################################################

IO_MCU_VERSION = $(shell cat $(BR2_EXTERNAL_OPENHC_PATH)/common/firmware/io-mcu/VERSION)
IO_MCU_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/common/firmware/io-mcu
IO_MCU_SITE_METHOD = local
IO_MCU_LICENSE = MIT
IO_MCU_DEPENDENCIES = host-arm-gnu-toolchain
IO_MCU_PARTS = tm4c1231d5 lm3s1162

define IO_MCU_BUILD_CMDS
	$(MAKE) -C $(@D) CROSS=$(HOST_DIR)/bin/arm-none-eabi- \
		PROTO=$(BR2_EXTERNAL_OPENHC_PATH)/common/kernel/drivers/gpio PYTHON=python3 all
endef

define IO_MCU_INSTALL_TARGET_CMDS
	for p in $(IO_MCU_PARTS); do \
		$(INSTALL) -D -m 0644 $(@D)/build/$$p/io-mcu-$$p.bin \
			$(TARGET_DIR)/lib/firmware/openhc/io-mcu-$$p.bin || exit 1; \
	done
endef

$(eval $(generic-package))
