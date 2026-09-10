################################################################################
#
# ths8200 — configure the THS8200 component video DAC over i2c-dev
#
# Local-source package, and it lives under board/hc800/ rather than packages/
# because it is HC-800 silicon, not a service: packages/ is for the daemons
# (ohc-iod, the web UI). ths8200.c lives next to this file. The register data is
# NOT compiled in — it is read at runtime from a .regs file installed alongside,
# so re-capturing it from hardware does not mean rebuilding the image.
#
################################################################################

THS8200_VERSION = 1.0
THS8200_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/hc800/packages/ths8200
THS8200_SITE_METHOD = local
THS8200_LICENSE = MIT

define THS8200_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall -Wextra \
		-o $(@D)/ths8200 $(@D)/ths8200.c
endef

# The register file comes from the BOARD tree, not this package — it is a
# property of the board's panel wiring, not of this tool.
define THS8200_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/ths8200 $(TARGET_DIR)/usr/bin/ths8200
	$(INSTALL) -D -m 0644 $(BR2_EXTERNAL_OPENHC_PATH)/hc800/video/ths8200-720p60.regs \
		$(TARGET_DIR)/opt/ohc/video/ths8200-720p60.regs
endef

$(eval $(generic-package))
