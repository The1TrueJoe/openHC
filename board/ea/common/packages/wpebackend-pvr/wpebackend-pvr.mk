################################################################################
#
# wpebackend-pvr
#
################################################################################

WPEBACKEND_PVR_VERSION = 0.1.0
WPEBACKEND_PVR_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/ea-common/packages/wpebackend-pvr
WPEBACKEND_PVR_SITE_METHOD = local
WPEBACKEND_PVR_LICENSE = MIT
WPEBACKEND_PVR_INSTALL_STAGING = YES

# sgx545-um supplies the EGL headers, and its eglplatform.h is already patched
# there to stop defaulting to X11. libwpe supplies the backend interface.
WPEBACKEND_PVR_DEPENDENCIES = libwpe sgx545-um

define WPEBACKEND_PVR_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) -C $(@D) \
		CC="$(TARGET_CC)" \
		WPE_CFLAGS="-I$(STAGING_DIR)/usr/include/wpe-1.0 -I$(STAGING_DIR)/usr/include" \
		EGL_CFLAGS= \
		WPE_LIBS=
endef

define WPEBACKEND_PVR_INSTALL_STAGING_CMDS
	$(MAKE) -C $(@D) install DESTDIR=$(STAGING_DIR) PREFIX=/usr
endef

define WPEBACKEND_PVR_INSTALL_TARGET_CMDS
	$(MAKE) -C $(@D) install DESTDIR=$(TARGET_DIR) PREFIX=/usr

	# Absent WPE_BACKEND in the environment libwpe loads
	# libWPEBackend-default.so.1. This is the only backend on the system,
	# so make it the default rather than requiring every caller to export
	# a variable to get a display at all.
	ln -sf libWPEBackend-pvr.so.1 \
		$(TARGET_DIR)/usr/lib/libWPEBackend-default.so.1
endef

$(eval $(generic-package))
