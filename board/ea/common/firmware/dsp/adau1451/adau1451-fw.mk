################################################################################
#
# adau1451-fw — openHC's SigmaDSP parameter file for the EA3's ADAU1451
#
# The build rule for THIS folder: firmware generated into the image from its
# source here, not a package (external.mk includes firmware/*/*/*.mk). Built
# from ea3.json by mkfw.py
# (the build host's python3, standard library only). The file is the kernel's
# own SigmaDSP firmware format: adau1451-c4 hands it to the stock sigmadsp core,
# which writes its tables at bind and turns its controls into ALSA controls.
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

define ADAU1451_FW_BUILD_CMDS
	python3 $(@D)/mkfw.py $(@D)/ea3.json $(@D)/adau1451-ea3.bin
endef

define ADAU1451_FW_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0644 $(@D)/adau1451-ea3.bin \
		$(TARGET_DIR)/lib/firmware/openhc/adau1451-ea3.bin
	$(INSTALL) -D -m 0644 $(@D)/ea3.json \
		$(TARGET_DIR)/usr/share/openhc/dsp/adau1451-ea3.json
endef

$(eval $(generic-package))
