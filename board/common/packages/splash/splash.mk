################################################################################
#
# splash — framebuffer boot splash
#
# A BASE FIRMWARE FEATURE, not a service — which is why it lives under
# board/common/ and not packages/. packages/ is for the daemons with APIs.
# splash.c lives next to this file. The banner text is
# produced on the device by figlet at boot (see S01splash), not baked in here,
# so the artwork is never a checked-in blob and the word can change at runtime.
#
################################################################################

SPLASH_VERSION = 1.0
SPLASH_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/common/packages/splash
SPLASH_SITE_METHOD = local
SPLASH_LICENSE = MIT
SPLASH_DEPENDENCIES = figlet

define SPLASH_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall -Wextra \
		-o $(@D)/splash $(@D)/splash.c
endef

define SPLASH_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/splash $(TARGET_DIR)/usr/bin/splash
endef

$(eval $(generic-package))
