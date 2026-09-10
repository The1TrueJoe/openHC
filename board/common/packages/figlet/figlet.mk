################################################################################
#
# figlet
#
# Upstream ships a hand-rolled Makefile, not autotools, so the cross settings
# go in on the command line. DEFAULTFONTDIR is compiled into the binary — it is
# where figlet looks when -d is not given — so it must be the target path, not
# a staging path.
#
# utf8.h wraps its prototypes in __BEGIN_DECLS/__END_DECLS, which come from
# <sys/cdefs.h> — a glibc/BSD extension musl does not ship. Defining them empty
# on the command line is a one-liner; a patch would be the tidier fix but it
# only applies on a fresh extract, and this is a two-macro papercut.
#
# Only slant and standard are installed: the full fonts/ directory is ~700K and
# openHC needs exactly one face for the splash.
#
################################################################################

FIGLET_VERSION = 2.2.5
FIGLET_SITE = $(call github,cmatsuoka,figlet,$(FIGLET_VERSION))
FIGLET_LICENSE = BSD-3-Clause
FIGLET_LICENSE_FILES = LICENSE

FIGLET_FONTDIR = /usr/share/figlet

define FIGLET_BUILD_CMDS
	$(MAKE) -C $(@D) \
		CC="$(TARGET_CC)" \
		LD="$(TARGET_CC)" \
		CFLAGS="$(TARGET_CFLAGS) -D__BEGIN_DECLS= -D__END_DECLS=" \
		LDFLAGS="$(TARGET_LDFLAGS)" \
		DEFAULTFONTDIR="$(FIGLET_FONTDIR)" \
		figlet
endef

define FIGLET_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/figlet $(TARGET_DIR)/usr/bin/figlet
	$(INSTALL) -D -m 0644 $(@D)/fonts/slant.flf \
		$(TARGET_DIR)$(FIGLET_FONTDIR)/slant.flf
	$(INSTALL) -D -m 0644 $(@D)/fonts/standard.flf \
		$(TARGET_DIR)$(FIGLET_FONTDIR)/standard.flf
endef

$(eval $(generic-package))
