################################################################################
#
# powervr-sgx-firmware
#
################################################################################

# The release's firmware package: keep it the same release as the submodule.
POWERVR_SGX_FIRMWARE_VERSION = 0.2.2
POWERVR_SGX_FIRMWARE_SOURCE = sgx545-firmware_$(POWERVR_SGX_FIRMWARE_VERSION)_all.deb
POWERVR_SGX_FIRMWARE_SITE = https://github.com/The1TrueJoe/powervr_sgx/releases/download/v$(POWERVR_SGX_FIRMWARE_VERSION)
POWERVR_SGX_FIRMWARE_LICENSE = MIT
POWERVR_SGX_FIRMWARE_FW = lib/firmware/powervr_sgx/sgx545_ukernel.bin

# A .deb is an ar archive: unpacked at build time, once the toolchain's ar exists.
define POWERVR_SGX_FIRMWARE_EXTRACT_CMDS
	cp $(POWERVR_SGX_FIRMWARE_DL_DIR)/$(POWERVR_SGX_FIRMWARE_SOURCE) $(@D)/
endef

define POWERVR_SGX_FIRMWARE_BUILD_CMDS
	cd $(@D) && $(TARGET_AR) x $(POWERVR_SGX_FIRMWARE_SOURCE) data.tar.xz
	$(XZCAT) $(@D)/data.tar.xz | $(TAR) -C $(@D) $(TAR_OPTIONS) -
	test -f $(@D)/$(POWERVR_SGX_FIRMWARE_FW)
endef

define POWERVR_SGX_FIRMWARE_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0644 $(@D)/$(POWERVR_SGX_FIRMWARE_FW) $(TARGET_DIR)/$(POWERVR_SGX_FIRMWARE_FW)
endef

$(eval $(generic-package))
