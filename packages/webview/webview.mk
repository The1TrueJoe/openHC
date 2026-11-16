################################################################################
#
# webview
#
################################################################################

WEBVIEW_VERSION = 0.1.0
WEBVIEW_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/../packages/webview
WEBVIEW_SITE_METHOD = local
WEBVIEW_LICENSE = MIT
# libglib2 is listed even though wpewebkit already pulls it in: this source
# includes <glib.h> and <glib-object.h> itself.
WEBVIEW_DEPENDENCIES = wpewebkit libglib2 host-pkgconf

define WEBVIEW_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) -C $(@D) CC="$(TARGET_CC)" \
		CFLAGS="$(TARGET_CFLAGS)" LDFLAGS="$(TARGET_LDFLAGS)"
endef

# The font aliases only where fontconfig reads them.
define WEBVIEW_INSTALL_TARGET_CMDS
	$(MAKE) -C $(@D) install DESTDIR=$(TARGET_DIR) PREFIX=/usr
	$(if $(BR2_PACKAGE_FONTCONFIG),$(INSTALL) -D -m 0644 $(@D)/fontconfig/99-openhc-ui.conf \
		$(TARGET_DIR)/etc/fonts/conf.d/99-openhc-ui.conf)
endef

define WEBVIEW_INSTALL_INIT_SYSV
	$(INSTALL) -D -m 0755 $(@D)/S95ohc-display $(TARGET_DIR)/etc/init.d/S95ohc-display
endef

$(eval $(generic-package))
