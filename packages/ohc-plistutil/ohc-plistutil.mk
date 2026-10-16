################################################################################
#
# ohc-plistutil -- libplist's plistutil for the BUILD host (host-ohc-plistutil).
#
# shairport-sync's AirPlay 2 build turns plists/get_info_response.xml into a
# binary plist with plistutil and embeds it; Buildroot packages libplist for the
# target only. It has to be libplist's own parser: that XML has a stray </dict>
# (left behind when a block was commented out), which libplist tolerates and
# stricter parsers reject — Python's plistlib raised, the codegen pipe swallowed
# it, an EMPTY plist was embedded, and every AirPlay 2 connection then failed
# with "generateInfoPlist plist not created!". Same release as Buildroot's
# target libplist.
#
################################################################################

OHC_PLISTUTIL_VERSION = 2.7.0
OHC_PLISTUTIL_SOURCE = libplist-$(OHC_PLISTUTIL_VERSION).tar.bz2
OHC_PLISTUTIL_SITE = https://github.com/libimobiledevice/libplist/releases/download/$(OHC_PLISTUTIL_VERSION)
OHC_PLISTUTIL_LICENSE = LGPL-2.1+ (library), GPL-2.0+ (tools)
OHC_PLISTUTIL_LICENSE_FILES = COPYING COPYING.LESSER
HOST_OHC_PLISTUTIL_CONF_OPTS = --without-cython --disable-static

$(eval $(host-autotools-package))
