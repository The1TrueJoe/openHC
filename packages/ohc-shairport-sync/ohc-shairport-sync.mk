################################################################################
#
# ohc-shairport-sync -- shairport-sync with AirPlay 2, at the development
# commit where several AirPlay 2 instances can share one host and address:
#   #2277 honour a set RTSP port for AirPlay 2 (it was forced to 7000)
#   #2273 a per-instance nqptp shared-memory name
#   #2278 retry the nqptp probe while nqptp creates that region
#   #2287 derive each instance's device id and nqptp name from its name
# with ohc-nqptp (nqptp#50) as the one shared timing daemon. Buildroot's own
# package is 4.3.7, before all of it; this is that recipe, updated for 5.x.
# Move to a release once one carries #2287.
#
################################################################################

OHC_SHAIRPORT_SYNC_VERSION = 8c146e25183a354ae96f91fa069c2e7173f75a71
OHC_SHAIRPORT_SYNC_SITE = https://github.com/mikebrady/shairport-sync.git
OHC_SHAIRPORT_SYNC_SITE_METHOD = git
OHC_SHAIRPORT_SYNC_LICENSE = MIT, BSD-3-Clause
OHC_SHAIRPORT_SYNC_LICENSE_FILES = LICENSES
# git clone, no configure
OHC_SHAIRPORT_SYNC_AUTORECONF = YES
# host-vim for xxd (the build embeds a plist as a C array).
OHC_SHAIRPORT_SYNC_DEPENDENCIES = alsa-lib libconfig popt host-pkgconf avahi openssl \
	libglib2 libplist libsodium libgcrypt ffmpeg util-linux host-vim

OHC_SHAIRPORT_SYNC_CONF_OPTS = \
	--with-alsa \
	--with-avahi --without-tinysvcmdns \
	--with-ssl=openssl \
	--with-airplay-2 \
	--with-metadata --with-metadata-pipe \
	--with-dbus-interface --with-mpris-interface \
	--without-libdaemon \
	--without-soxr \
	--without-convolution \
	--without-mqtt-client \
	--without-systemd-startup --without-systemv-startup \
	--without-create-user-group \
	--without-configfiles

# The build turns plists/get_info_response.xml into a binary plist with
# libplist's `plistutil` on the BUILD host, which Buildroot does not package
# (libplist is target-only). Python's plistlib writes the same format; a shim
# named plistutil, first on the configure and make PATH, stands in for it.
OHC_SHAIRPORT_SYNC_SHIM = $(@D)/.ohc-host-bin
define OHC_SHAIRPORT_SYNC_PLISTUTIL_SHIM
	mkdir -p $(OHC_SHAIRPORT_SYNC_SHIM)
	printf '%s\n' '#!/bin/sh' \
		'# plistutil -i <xml> : the binary plist on stdout (ohc-shairport-sync.mk)' \
		'exec python3 -c "import plistlib,sys; plistlib.dump(plistlib.load(open(sys.argv[1],\"rb\")), sys.stdout.buffer, fmt=plistlib.FMT_BINARY)" "$$2"' \
		> $(OHC_SHAIRPORT_SYNC_SHIM)/plistutil
	chmod 0755 $(OHC_SHAIRPORT_SYNC_SHIM)/plistutil
endef
OHC_SHAIRPORT_SYNC_PRE_CONFIGURE_HOOKS += OHC_SHAIRPORT_SYNC_PLISTUTIL_SHIM
OHC_SHAIRPORT_SYNC_CONF_ENV += PATH="$(OHC_SHAIRPORT_SYNC_SHIM):$(BR_PATH)"
OHC_SHAIRPORT_SYNC_MAKE_ENV += PATH="$(OHC_SHAIRPORT_SYNC_SHIM):$(BR_PATH)"

define OHC_SHAIRPORT_SYNC_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/shairport-sync $(TARGET_DIR)/usr/bin/shairport-sync
	$(INSTALL) -D -m 0644 $(@D)/scripts/shairport-sync-dbus-policy.conf \
		$(TARGET_DIR)/etc/dbus-1/system.d/shairport-sync-dbus.conf
	$(INSTALL) -D -m 0644 $(@D)/scripts/shairport-sync-mpris-policy.conf \
		$(TARGET_DIR)/etc/dbus-1/system.d/shairport-sync-mpris.conf
endef

$(eval $(autotools-package))
