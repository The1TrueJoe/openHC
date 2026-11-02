################################################################################
#
# adau1451-fw — openHC's SigmaDSP firmware file for the EA3's ADAU1451
#
# The build rule for THIS folder: firmware generated into the image from its
# source here, not a package (external.mk includes firmware/*/*/*.mk). Built
# from ea3.json by mkfw.py
# (the build host's python3, standard library only). The file is the kernel's
# own SigmaDSP firmware format: adau1451-c4 hands it to the stock sigmadsp core,
# which writes its tables at bind and turns its controls into ALSA controls.
#
# With BR2_PACKAGE_ADAU1451_FW_PROGRAM set (a folder holding export2program.py's
# program.txt and params.json, which CI builds in the same run and never
# commits), a second file is made too: adau1451-ea3-openhc.bin, from
# ea3-openhc.json, carrying the register init, openHC's own program and its
# cells. The device tree still names adau1451-ea3.bin, so it is not loaded at
# boot yet.
#
# The profile is installed too: audiod reads its knob list from it, so the
# web UI shows exactly the knobs the firmware has.
#
################################################################################

ADAU1451_FW_VERSION = 1.0
ADAU1451_FW_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/ea/common/firmware/dsp/adau1451
ADAU1451_FW_SITE_METHOD = local
ADAU1451_FW_LICENSE = GPL-2.0
ADAU1451_FW_INSTALL_STAGING = NO

ADAU1451_FW_PROGRAM = $(call qstrip,$(BR2_PACKAGE_ADAU1451_FW_PROGRAM))

define ADAU1451_FW_BUILD_CMDS
	python3 $(@D)/mkfw.py $(@D)/ea3.json $(@D)/adau1451-ea3.bin
endef

define ADAU1451_FW_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0644 $(@D)/adau1451-ea3.bin \
		$(TARGET_DIR)/lib/firmware/openhc/adau1451-ea3.bin
	$(INSTALL) -D -m 0644 $(@D)/ea3.json \
		$(TARGET_DIR)/usr/share/openhc/dsp/adau1451-ea3.json
endef

ifneq ($(ADAU1451_FW_PROGRAM),)
define ADAU1451_FW_BUILD_OPENHC
	python3 $(@D)/mkfw.py $(@D)/ea3-openhc.json $(@D)/adau1451-ea3-openhc.bin \
		--program $(ADAU1451_FW_PROGRAM)/program.txt \
		--params $(ADAU1451_FW_PROGRAM)/params.json
endef
ADAU1451_FW_POST_BUILD_HOOKS += ADAU1451_FW_BUILD_OPENHC

define ADAU1451_FW_INSTALL_OPENHC
	$(INSTALL) -D -m 0644 $(@D)/adau1451-ea3-openhc.bin \
		$(TARGET_DIR)/lib/firmware/openhc/adau1451-ea3-openhc.bin
endef
ADAU1451_FW_POST_INSTALL_TARGET_HOOKS += ADAU1451_FW_INSTALL_OPENHC
endif

# generic-package names a package after its FOLDER (pkg-utils.mk: pkgname is
# the last component of the .mk's directory), and this folder is the part,
# adau1451, not the rule. Through generic-package these ADAU1451_FW_* settings
# were never read and the firmware never built into an image; name it here.
$(eval $(call inner-generic-package,adau1451-fw,ADAU1451_FW,ADAU1451_FW,target))
