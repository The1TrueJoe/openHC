################################################################################
#
# ohc-splash — framebuffer boot splash
#
# A BASE FIRMWARE FEATURE, not a service — which is why it lives under
# board/common/ and not packages/. packages/ is for the daemons with APIs.
# splash.c lives next to this file. The banner text is
# produced on the device by figlet at boot (see S01splash), not baked in here,
# so the artwork is never a checked-in blob and the word can change at runtime.
#
################################################################################

OHC_SPLASH_VERSION = 1.0
OHC_SPLASH_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/common/packages/ohc-splash
OHC_SPLASH_SITE_METHOD = local
OHC_SPLASH_LICENSE = MIT
OHC_SPLASH_DEPENDENCIES = figlet

define OHC_SPLASH_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall -Wextra \
		-o $(@D)/ohc-splash $(@D)/splash.c
endef

define OHC_SPLASH_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/ohc-splash $(TARGET_DIR)/usr/bin/ohc-splash
endef

$(eval $(generic-package))
