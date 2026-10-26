################################################################################
#
# em357-ncp-firmware — the EmberZNet NCP image ohc-zigbee keeps the EM357 on
# (board/common/firmware: the boards' co-processor firmware lives with the boards)
#
# Every openHC board with Zigbee (CA-1, the EA family, the HC-800) carries an
# EM357 network co-processor. Control4 reflashes it from images on its rootfs;
# openHC does the same: at boot ohc-zigbee reads the running EmberZNet version
# and, when older, uploads this image through the chip's serial bootloader
# (packages/zigbee/src/ncp.rs). Installed with a `.version` file beside it,
# which is what that comparison reads.
#
# EmberZNet 6.7.8, EZSP protocol 8 — the last EmberZNet line for the EM35x —
# the UART NCP with software (XON/XOFF) flow control, as published for the
# Telegesis ETRX357 (an EM357 on the same SC1 PB1/PB2 UART every EM357 NCP
# uses). Pinned to a commit and checked by sha256, not vendored: a Silicon Labs
# binary, redistributed by walthowd/husbzb-firmware.
#
################################################################################

EM357_NCP_FIRMWARE_VERSION = d6a512ccb2d770d4fdabdc27805ddd68fde96988
EM357_NCP_FIRMWARE_SITE = https://raw.githubusercontent.com/walthowd/husbzb-firmware/$(EM357_NCP_FIRMWARE_VERSION)
EM357_NCP_FIRMWARE_SOURCE = em357-v678-ncp-uart-sw.ebl
EM357_NCP_FIRMWARE_STACK = 6.7.8.0
EM357_NCP_FIRMWARE_LICENSE = Silicon Labs EmberZNet NCP binary (redistributed by walthowd/husbzb-firmware)

# A single file, not an archive.
define EM357_NCP_FIRMWARE_EXTRACT_CMDS
	cp $(EM357_NCP_FIRMWARE_DL_DIR)/$(EM357_NCP_FIRMWARE_SOURCE) $(@D)/
endef

define EM357_NCP_FIRMWARE_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0644 $(@D)/$(EM357_NCP_FIRMWARE_SOURCE) \
		$(TARGET_DIR)/usr/share/ohc/zigbee/em357-ncp.ebl
	printf '%s\n' $(EM357_NCP_FIRMWARE_STACK) > \
		$(TARGET_DIR)/usr/share/ohc/zigbee/em357-ncp.ebl.version
endef

$(eval $(generic-package))
