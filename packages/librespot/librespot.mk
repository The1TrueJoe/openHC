################################################################################
#
# librespot -- Spotify Connect receiver
#
# Not in Buildroot, so openHC carries it. Built with Buildroot's cargo
# infrastructure rather than the host-side packages/build.sh route the other
# openHC Rust daemons use: librespot links against the TARGET's alsa-lib, and
# host-side cross-building would mean reproducing Buildroot's sysroot by hand.
#
################################################################################

LIBRESPOT_VERSION = v0.4.2
LIBRESPOT_SITE = https://github.com/librespot-org/librespot.git
LIBRESPOT_SITE_METHOD = git

# git, not the github() archive helper, and deliberately no .hash file.
#
# GitHub's auto-generated /archive/ tarballs are NOT byte-stable -- the same URL
# can come back with different compression, so a pinned sha256 fails at random.
# Measured here: fetching the exact URL the github() macro builds gave
# cc8cb81b..., while Buildroot's own fetch of the same URL got 4a9fdde3...
# The git method clones the pinned tag and repacks it deterministically instead.
LIBRESPOT_LICENSE = MIT
LIBRESPOT_LICENSE_FILES = LICENSE
LIBRESPOT_DEPENDENCIES = host-pkgconf alsa-lib

# Only the ALSA backend. The defaults drag in pulseaudio, portaudio, jackaudio
# and gstreamer -- none of which exist on this image, and each of which turns a
# missing header into a confusing Rust link error rather than a clear one.
LIBRESPOT_CARGO_BUILD_OPTS = --no-default-features --features alsa-backend

define LIBRESPOT_INSTALL_INIT_SYSV
	$(INSTALL) -D -m 0755 $(LIBRESPOT_PKGDIR)/S95librespot \
		$(TARGET_DIR)/etc/init.d/S95librespot
endef

$(eval $(cargo-package))
