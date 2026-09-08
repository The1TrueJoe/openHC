################################################################################
#
# sgx545-um — PowerVR SGX545 userspace: EGL 1.4 + OpenGL ES 2.0
#
# Intel's Cedarview DDK 1.7.862890 release. Cedarview (GMA 3600/3650) and the
# CE5300 carry the same SGX545 core, and this is the only DDK 1.7 userspace
# that exists for it — which matters, because the kernel driver checks the DDK
# version on connect and refuses a mismatch. Keep this and sgx545-ce on the
# same DDK.
#
# Fetched, not vendored: these are proprietary binaries. Intel's licence allows
# redistribution in binary form with the notice, and forbids reverse
# engineering them. The hash in sgx545-um.hash is the one carried by the
# PGP-signed .dsc published alongside the tarball.
#
# Needs a glibc toolchain — the blobs are glibc-linked and cannot be loaded by
# musl at all. board/ea-common/ea-common_defconfig selects glibc for exactly
# this reason.
#
################################################################################

SGX545_UM_VERSION = 20120717
SGX545_UM_SOURCE = cedarview-graphics-drivers_$(SGX545_UM_VERSION).orig.tar.gz
SGX545_UM_SITE = https://old-releases.ubuntu.com/ubuntu/pool/multiverse/c/cedarview-graphics-drivers
SGX545_UM_LICENSE = Intel/Imagination binary redistribution
SGX545_UM_LICENSE_FILES = usr/share/doc/powervr/license.txt
SGX545_UM_REDISTRIBUTE = NO

SGX545_UM_DEPENDENCIES = libdrm sgx545-ce
SGX545_UM_INSTALL_STAGING = YES

# Tell Buildroot this package is the EGL/GLES provider, so anything that wants
# GLES can depend on the virtual libegl/libgles rather than on us by name.
# Same shape as ti-sgx-um, which is the in-tree precedent for a binary SGX
# userspace.
SGX545_UM_PROVIDES = libegl libgles

# Nothing to build: prebuilt binaries.
SGX545_UM_INSTALL_TARGET_OPTS =

define SGX545_UM_BUILD_CMDS
endef

# cp -a throughout: the tarball ships the usual libFOO.so -> libFOO.so.1 ->
# libFOO.so.1.7.862890 chains and the loader needs them intact.
define SGX545_UM_INSTALL_STAGING_CMDS
	mkdir -p $(STAGING_DIR)/usr/lib $(STAGING_DIR)/usr/include
	cp -a $(@D)/usr/lib/*.so* $(STAGING_DIR)/usr/lib/
	cp -a $(@D)/usr/include/EGL $(@D)/usr/include/GLES $(@D)/usr/include/GLES2 \
		$(@D)/usr/include/KHR $(STAGING_DIR)/usr/include/

	# The DDK's eglplatform.h decides the native types by platform, and for
	# __unix__ it defaults to X11 -- it includes <X11/Xlib.h> and typedefs
	# EGLNativeWindowType to Window. There is no X server and no X headers
	# here, so every package that so much as includes <EGL/egl.h> fails to
	# compile; cairo-gl is the first to hit it, WPE would be next.
	#
	# Upstream's own escape hatch is MESA_EGL_NO_X11_HEADERS, which selects
	# the plain-integer typedefs -- and those are exactly what the LinuxFB
	# WSEGL wants, since it takes no window handle at all. Define it in the
	# installed header rather than requiring every consumer to pass
	# -DMESA_EGL_NO_X11_HEADERS: the header we ship should be correct for
	# the system we ship it on, not correct only for callers who know this.
	$(SED) '1i\
#define MESA_EGL_NO_X11_HEADERS 1 /* openHC: no X11 on this system */' \
		$(STAGING_DIR)/usr/include/EGL/eglplatform.h
	$(INSTALL) -D -m 0644 $(@D)/usr/lib/pkgconfig/egl.pc \
		$(STAGING_DIR)/usr/lib/pkgconfig/egl.pc
	$(INSTALL) -D -m 0644 $(@D)/usr/lib/pkgconfig/glesv2.pc \
		$(STAGING_DIR)/usr/lib/pkgconfig/glesv2.pc

	# The DDK's egl.pc was written for its X11/DRI window system:
	#
	#   Requires.private: libdrm >= 2.4.25 dri2proto >= 2.4 \
	#                     glproto >= 1.4.13 x11 xext xdamage xfixes xxf86vm
	#
	# We ship the LinuxFB WSEGL instead (see INSTALL_TARGET_CMDS), so none
	# of those exist. Requires.private looks like it should be harmless for
	# a shared build, but pkgconf resolves it even for a plain --cflags and
	# ERRORS when it cannot, so the breakage is not confined to EGL's own
	# consumers:
	# cairo built with cairo-gl gets "Requires: egl" in its own cairo.pc,
	# and from then on every `pkg-config cairo` in the tree errors out with
	# "Package 'x11', required by 'egl', not found". That is what stops
	# harfbuzz finding a cairo that demonstrably compiled minutes earlier.
	#
	# The libraries themselves link nothing outside the DDK in this
	# configuration, so the dependency list is not merely unsatisfiable, it
	# is wrong. Drop it.
	$(SED) '/^Requires\(\.private\)\?:/d' \
		$(STAGING_DIR)/usr/lib/pkgconfig/egl.pc \
		$(STAGING_DIR)/usr/lib/pkgconfig/glesv2.pc
endef

define SGX545_UM_INSTALL_TARGET_CMDS
	mkdir -p $(TARGET_DIR)/usr/lib
	cp -a $(@D)/usr/lib/*.so* $(TARGET_DIR)/usr/lib/

	# libIMGegl dlopens the window-system backend by the FIXED name
	# libpvrPVR2D_DRIWSEGL.so, which is the X11/DRI one. openHC has no X
	# server; it draws to /dev/fb0. All five WSEGL backends export the same
	# WSEGL_GetFunctionTablePointer, so installing the framebuffer backend
	# under the name libIMGegl asks for is the supported way to select it.
	ln -sf libpvrPVR2D_LINUXFBWSEGL.so.1.7.862890 \
		$(TARGET_DIR)/usr/lib/libpvrPVR2D_DRIWSEGL.so

	$(INSTALL) -D -m 0644 $(SGX545_UM_PKGDIR)/powervr.ini \
		$(TARGET_DIR)/etc/powervr.ini

	# The bus id the DDK looks for is wrong on this board; see the shim.
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -shared -fPIC \
		-o $(@D)/busidshim.so \
		$(BR2_EXTERNAL_OPENHC_PATH)/ea-common/packages/sgx545-ce/tools/busidshim.c -ldl
	$(INSTALL) -D -m 0644 $(@D)/busidshim.so \
		$(TARGET_DIR)/usr/lib/sgx-busidshim.so

	# ...and preload it system-wide rather than leaving every caller to
	# remember an LD_PRELOAD. This is not a convenience: libsrv_um calls
	# drmOpen(NULL, "pci:0000:00:02.0"), and because the name argument is
	# NULL libdrm has no name-based fallback to reach -- the open simply
	# fails, with no ioctl issued and nothing in dmesg. Without the shim
	# EGL does not initialise at all, so a GLES program that is not
	# preloaded is not a degraded program, it is a broken one.
	#
	# It therefore belongs to the system and not to any one program. WPE
	# in particular spawns a separate WebProcess to do its rendering, and
	# that process is the one that needs the shim; ld.so.preload covers it
	# without webview having to know how WebKit spawns children.
	#
	# The shim overrides exactly one symbol, drmOpen, which nothing else on
	# this system calls, and honours SGX_BUSID if a board ever needs a
	# different address.
	echo /usr/lib/sgx-busidshim.so > $(TARGET_DIR)/etc/ld.so.preload
endef

$(eval $(generic-package))
