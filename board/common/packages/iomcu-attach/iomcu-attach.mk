################################################################################
#
# iomcu-attach — attach the IO microcontroller line discipline
#
# A BASE FIRMWARE FEATURE, not a service, so it lives under board/common/ rather
# than packages/ — packages/ is for the daemons with APIs.
#
# It exists instead of util-linux's ldattach, which would do the job but only by
# pulling in BR2_PACKAGE_UTIL_LINUX_BINARIES: roughly fifty binaries, for one
# ioctl, on an image whose size the project takes seriously.
#
################################################################################

IOMCU_ATTACH_VERSION = 1.0
IOMCU_ATTACH_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/common/packages/iomcu-attach
IOMCU_ATTACH_SITE_METHOD = local
IOMCU_ATTACH_LICENSE = MIT

define IOMCU_ATTACH_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall -Wextra \
		-o $(@D)/iomcu-attach $(@D)/iomcu-attach.c
endef

define IOMCU_ATTACH_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/iomcu-attach $(TARGET_DIR)/usr/bin/iomcu-attach
endef

$(eval $(generic-package))
