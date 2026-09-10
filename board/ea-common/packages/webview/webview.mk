################################################################################
#
# webview
#
################################################################################

WEBVIEW_VERSION = 0.1.0
WEBVIEW_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/ea-common/packages/webview
WEBVIEW_SITE_METHOD = local
WEBVIEW_LICENSE = MIT
# libglib2 is listed even though wpewebkit already pulls it in: this source
# includes <glib.h> and <glib-object.h> itself, and depending on someone
# else's dependency is how wpewebkit ended up with no ordering guarantee on
# libwpe (see board/buildroot-patches/0001).
WEBVIEW_DEPENDENCIES = wpewebkit libwpe libglib2 host-pkgconf

define WEBVIEW_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) -C $(@D) CC="$(TARGET_CC)" \
		CFLAGS="$(TARGET_CFLAGS)" LDFLAGS="$(TARGET_LDFLAGS)"
endef

define WEBVIEW_INSTALL_TARGET_CMDS
	$(MAKE) -C $(@D) install DESTDIR=$(TARGET_DIR) PREFIX=/usr
endef

$(eval $(generic-package))
