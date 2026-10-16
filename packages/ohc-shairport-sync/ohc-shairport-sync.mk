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
# The build embeds plists/get_info_response.xml as a C array of its binary
# form: libplist's plistutil (host-ohc-plistutil, see that package for why it
# must be libplist's) piped into xxd (host-vim).
OHC_SHAIRPORT_SYNC_DEPENDENCIES = alsa-lib libconfig popt host-pkgconf avahi openssl \
	libglib2 libplist libsodium libgcrypt ffmpeg util-linux host-vim host-ohc-plistutil

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

# Always regenerate the embedded plist: a build tree restored from cache keeps
# the generated get_info_response.{c,h} (newer than the .xml), so make never
# reruns the codegen — which kept the empty one from the build before
# host-ohc-plistutil existed.
define OHC_SHAIRPORT_SYNC_REGENERATE_INFO_PLIST
	rm -f $(@D)/plists/get_info_response.c $(@D)/plists/get_info_response.h
endef
OHC_SHAIRPORT_SYNC_PRE_BUILD_HOOKS += OHC_SHAIRPORT_SYNC_REGENERATE_INFO_PLIST

# The codegen is a pipe, so a plistutil failure does not fail the build —
# it embeds an empty plist and every AirPlay 2 connection fails at runtime.
# Refuse to install one that is not a binary plist ("bplist00").
define OHC_SHAIRPORT_SYNC_CHECK_INFO_PLIST
	grep -q '0x62, 0x70, 0x6c, 0x69, 0x73, 0x74' $(@D)/plists/get_info_response.c || \
		{ echo "ohc-shairport-sync: plists/get_info_response.c holds no binary plist" >&2; exit 1; }
endef
OHC_SHAIRPORT_SYNC_POST_BUILD_HOOKS += OHC_SHAIRPORT_SYNC_CHECK_INFO_PLIST

define OHC_SHAIRPORT_SYNC_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/shairport-sync $(TARGET_DIR)/usr/bin/shairport-sync
	$(INSTALL) -D -m 0644 $(@D)/scripts/shairport-sync-dbus-policy.conf \
		$(TARGET_DIR)/etc/dbus-1/system.d/shairport-sync-dbus.conf
	$(INSTALL) -D -m 0644 $(@D)/scripts/shairport-sync-mpris-policy.conf \
		$(TARGET_DIR)/etc/dbus-1/system.d/shairport-sync-mpris.conf
endef

$(eval $(autotools-package))
