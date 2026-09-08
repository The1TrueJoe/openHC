################################################################################
#
# ohc-ths8200 — configure the THS8200 component video DAC over i2c-dev
#
# Local-source package: ths8200.c lives next to this file. The register data is
# NOT compiled in — it is read at runtime from a .regs file installed alongside,
# so re-capturing it from hardware does not mean rebuilding the image.
#
################################################################################

OHC_THS8200_VERSION = 1.0
OHC_THS8200_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/../packages/ohc-ths8200
OHC_THS8200_SITE_METHOD = local
OHC_THS8200_LICENSE = MIT

define OHC_THS8200_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall -Wextra \
		-o $(@D)/ohc-ths8200 $(@D)/ths8200.c
endef

# The register file comes from the BOARD tree, not this package — it is a
# property of the board's panel wiring, not of this tool.
define OHC_THS8200_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/ohc-ths8200 $(TARGET_DIR)/usr/bin/ohc-ths8200
	$(INSTALL) -D -m 0644 $(BR2_EXTERNAL_OPENHC_PATH)/hc800/video/ths8200-720p60.regs \
		$(TARGET_DIR)/opt/ohc/video/ths8200-720p60.regs
endef

$(eval $(generic-package))
