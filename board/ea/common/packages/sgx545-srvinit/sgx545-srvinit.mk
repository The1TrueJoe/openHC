################################################################################
#
# sgx545-srvinit — open bring-up for the PowerVR SGX545 (no libsrv_init)
#
# The program is ours, built from the sgx545-ce submodule's srvinit/ + libpvr/
# against the kernel driver's own headers (libpvr/abi/flags.sh holds the
# driver build's defines, so the bridge structs match the in-tree pvrsrvkm).
#
# The microkernel it uploads is Imagination's and is not committed anywhere:
# the package's SOURCE is Intel's Cedarview DDK tarball, the same file and hash
# sgx545-um downloads, and srvinit/fetch-firmware.sh cuts the 53,096-byte
# microkernel out of its libsrv_init (sha256-checked on the way in and out).
#
# Like every package under board/*/*/packages/, build.sh forces a rebuild each
# build, so a submodule bump reaches the binary even from a cached tree.
#
################################################################################

SGX545_SRVINIT_VERSION = 20120717
SGX545_SRVINIT_SOURCE = cedarview-graphics-drivers_$(SGX545_SRVINIT_VERSION).orig.tar.gz
SGX545_SRVINIT_SITE = https://old-releases.ubuntu.com/ubuntu/pool/multiverse/c/cedarview-graphics-drivers
SGX545_SRVINIT_LICENSE = GPL-2.0 (driver headers), Intel/Imagination binary redistribution (microkernel)
SGX545_SRVINIT_LICENSE_FILES = usr/share/doc/powervr/license.txt
SGX545_SRVINIT_REDISTRIBUTE = NO

SGX545_SRVINIT_CE = $(BR2_EXTERNAL_OPENHC_PATH)/ea/common/sgx545-ce
SGX545_SRVINIT_FW = sgx545-ukernel-1.7.862890.bin

define SGX545_SRVINIT_BUILD_CMDS
	R=$(SGX545_SRVINIT_CE); . $$R/libpvr/abi/flags.sh; \
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -Wall $$DEFS $$INC -I$$R/libpvr \
		-o $(@D)/srvinit $$R/srvinit/srvinit.c $$R/libpvr/pvr.c
	sh $(SGX545_SRVINIT_CE)/srvinit/fetch-firmware.sh $(@D)/usr/lib $(@D)/$(SGX545_SRVINIT_FW)
endef

define SGX545_SRVINIT_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/srvinit $(TARGET_DIR)/usr/sbin/srvinit
	$(INSTALL) -D -m 0644 $(@D)/$(SGX545_SRVINIT_FW) \
		$(TARGET_DIR)/lib/firmware/$(SGX545_SRVINIT_FW)
endef

$(eval $(generic-package))
