################################################################################
#
# brcmfmac-bcm4319 — the kernel's brcmfmac, out of tree, with the BCM4319 added
#
# No source of its own: brcmfmac is copied from the kernel being built, so a
# kernel bump brings its brcmfmac along and only bcm4319.diff has to apply.
#
################################################################################

BRCMFMAC_BCM4319_VERSION = $(LINUX_VERSION)
BRCMFMAC_BCM4319_SOURCE =
BRCMFMAC_BCM4319_LICENSE = ISC
BRCMFMAC_BCM4319_DEPENDENCIES = linux
BRCMFMAC_BCM4319_MODULE_SUBDIRS = brcmfmac

BRCMFMAC_BCM4319_SRC = $(LINUX_DIR)/drivers/net/wireless/broadcom/brcm80211

define BRCMFMAC_BCM4319_COPY_DRIVER
	rm -rf $(@D)/brcmfmac $(@D)/include
	cp -a $(BRCMFMAC_BCM4319_SRC)/brcmfmac $(BRCMFMAC_BCM4319_SRC)/include $(@D)/
	find $(@D) \( -name '*.o' -o -name '*.ko' -o -name '*.mod*' -o -name '.*.cmd' \) -delete
	patch -d $(@D) -p1 --no-backup-if-mismatch < $(BRCMFMAC_BCM4319_PKGDIR)/bcm4319.diff
endef
BRCMFMAC_BCM4319_PRE_CONFIGURE_HOOKS += BRCMFMAC_BCM4319_COPY_DRIVER

# Ours replaces the in-tree module, which depmod would otherwise prefer.
define BRCMFMAC_BCM4319_DROP_INTREE
	find $(TARGET_DIR)/lib/modules -type d -path '*/kernel/drivers/net/wireless/broadcom/brcm80211/brcmfmac' \
		-exec rm -rf {} +
endef
BRCMFMAC_BCM4319_POST_INSTALL_TARGET_HOOKS += BRCMFMAC_BCM4319_DROP_INTREE

$(eval $(kernel-module))
$(eval $(generic-package))
