################################################################################
#
# alsa-ohceq — per-output tone (bass, treble, balance) as an ALSA plugin
#
# Built as an alsa-lib PCM module (libasound_module_pcm_ohceq.so), found by
# alsa-lib in its plugin directory when asound.conf names `type ohceq`. -DPIC
# is not optional: without it alsa-lib's SND_PCM_PLUGIN_SYMBOL emits the
# static-linking glue (snd_dlsym_start), and the module fails to load with
# "symbol not found" (libtool defines it for alsa-plugins; we have no libtool).
#
################################################################################

ALSA_OHCEQ_VERSION = 1.0
ALSA_OHCEQ_SITE = $(BR2_EXTERNAL_OPENHC_PATH)/../packages/alsa-ohceq
ALSA_OHCEQ_SITE_METHOD = local
ALSA_OHCEQ_LICENSE = MIT
ALSA_OHCEQ_DEPENDENCIES = alsa-lib

define ALSA_OHCEQ_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -shared -fPIC -DPIC -Wall -Wextra \
		-o $(@D)/libasound_module_pcm_ohceq.so $(@D)/ohceq.c -lasound -lm
endef

define ALSA_OHCEQ_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/libasound_module_pcm_ohceq.so \
		$(TARGET_DIR)/usr/lib/alsa-lib/libasound_module_pcm_ohceq.so
endef

$(eval $(generic-package))
