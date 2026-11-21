################################################################################
#
# zwave-js-config
#
# Z-Wave JS's device database: one JSON file per device, with its name and its
# configuration parameters (labels, ranges, options). zwave-bridge reads them as
# published ([devices] dir, set by S92ohczwave), so a node is named and its
# parameters can be set by name. The npm tarball is the release; only its
# config/devices tree is installed. Data only, nothing to build.
#
################################################################################

ZWAVE_JS_CONFIG_VERSION = 15.32.4
ZWAVE_JS_CONFIG_SOURCE = config-$(ZWAVE_JS_CONFIG_VERSION).tgz
ZWAVE_JS_CONFIG_SITE = https://registry.npmjs.org/@zwave-js/config/-/
ZWAVE_JS_CONFIG_LICENSE = MIT

define ZWAVE_JS_CONFIG_INSTALL_TARGET_CMDS
	rm -rf $(TARGET_DIR)/usr/share/zwave-js-config
	mkdir -p $(TARGET_DIR)/usr/share/zwave-js-config
	cp -a $(@D)/config/devices $(TARGET_DIR)/usr/share/zwave-js-config/devices
endef

$(eval $(generic-package))
