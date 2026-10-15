################################################################################
#
# ohc-nqptp -- nqptp at the development commit that serves several AirPlay 2
# instances on one host (nqptp#50, per-client shared memory). Buildroot ships
# 1.2.4; the 1.2.x releases serve one client. Move to a release once one
# carries #50.
#
################################################################################

OHC_NQPTP_VERSION = 0a88cd7ca9325f023ca5d891714695167672bdb9
OHC_NQPTP_SITE = https://github.com/mikebrady/nqptp.git
OHC_NQPTP_SITE_METHOD = git
OHC_NQPTP_LICENSE = GPL-2.0+
OHC_NQPTP_LICENSE_FILES = LICENSE
# git clone, no configure
OHC_NQPTP_AUTORECONF = YES
OHC_NQPTP_DEPENDENCIES = host-pkgconf

define OHC_NQPTP_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/nqptp $(TARGET_DIR)/usr/bin/nqptp
endef

$(eval $(autotools-package))
