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

LIBRESPOT_VERSION = v0.8.0
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
#
# v0.8.0 (was v0.4.2, the last release on Spotify's retired API). 0.6+ made the
# TLS stack and the zeroconf responder explicit features, so with
# --no-default-features both must be named or the build fails / the device is
# never discoverable:
#   with-libmdns            self-contained mDNS responder (no D-Bus/avahi needed)
#   rustls-tls-webpki-roots pure-Rust TLS with Mozilla's roots compiled in, so no
#                           OpenSSL link and no dependence on ca-certificates
# 0.8.0 needs Rust 1.85 (edition 2024) — Buildroot 2026.02.3 ships 1.88; the
# 2024.02 tree's 1.74 is why this sat on 0.4.2.
LIBRESPOT_CARGO_FEATURES = --no-default-features \
	--features alsa-backend,with-libmdns,rustls-tls-webpki-roots
LIBRESPOT_CARGO_BUILD_OPTS = $(LIBRESPOT_CARGO_FEATURES)
# The install step is a separate `cargo install`, which compiles again with
# its OWN options: without these it falls back to the default features
# (native-tls -> openssl-sys) and fails on a box with no OpenSSL.
LIBRESPOT_CARGO_INSTALL_OPTS = $(LIBRESPOT_CARGO_FEATURES)

# Link DYNAMICALLY. Rust's *-linux-musl targets default to +crt-static, which
# makes the linker look for libasound.a — Buildroot ships only the shared
# alsa-lib, so the link fails with "cannot find -lasound". librespot is a normal
# dynamically linked target binary like any other Buildroot package; say so.
LIBRESPOT_CARGO_ENV = \
	CARGO_TARGET_$(call UPPERCASE,$(RUSTC_TARGET_NAME))_RUSTFLAGS="-C target-feature=-crt-static"

define LIBRESPOT_INSTALL_INIT_SYSV
	$(INSTALL) -D -m 0755 $(LIBRESPOT_PKGDIR)/S95librespot \
		$(TARGET_DIR)/etc/init.d/S95librespot
endef

$(eval $(cargo-package))
