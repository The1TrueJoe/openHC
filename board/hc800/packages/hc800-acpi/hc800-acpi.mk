################################################################################
#
# hc800-acpi — the HC-800's ACPI table upgrade (board/hc800/acpi/*.asl)
#
# The sources live with the board, like a device tree would; this package only
# compiles them with the host iasl and packs them the way the kernel's early
# table-upgrade code reads them: an UNCOMPRESSED newc cpio holding
# kernel/firmware/acpi/<table>.aml, placed in front of the initrd by
# post-image.sh. Nothing is installed on the target.
#
################################################################################

HC800_ACPI_VERSION = 1.0
HC800_ACPI_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/hc800/acpi
HC800_ACPI_SITE_METHOD = local
HC800_ACPI_LICENSE = GPL-2.0
HC800_ACPI_DEPENDENCIES = host-acpica
HC800_ACPI_INSTALL_TARGET = NO
HC800_ACPI_INSTALL_IMAGES = YES

# -we: a warning in a table the kernel will trust at boot is a build failure.
define HC800_ACPI_BUILD_CMDS
	rm -rf $(@D)/cpio && mkdir -p $(@D)/cpio/kernel/firmware/acpi
	for f in $(@D)/*.asl; do \
		$(HOST_DIR)/bin/iasl -we -p $(@D)/cpio/kernel/firmware/acpi/$$(basename $$f .asl) $$f \
			|| exit 1; \
	done
	cd $(@D)/cpio && find kernel | LC_ALL=C sort | cpio -o -H newc -R 0:0 --quiet \
		> $(@D)/acpi-tables.cpio
endef

define HC800_ACPI_INSTALL_IMAGES_CMDS
	$(INSTALL) -D -m 0644 $(@D)/acpi-tables.cpio $(BINARIES_DIR)/acpi-tables.cpio
endef

$(eval $(generic-package))
