/* SPDX-License-Identifier: GPL-2.0 */
/*
 * The Intel CE5300 HDMI transmitter's audio, a child device of its DRM bridge
 * driver (drivers/gpu/drm/ce5300/ce5300-hdmi.c). The audio driver takes no
 * symbols from the bridge: everything it needs is in the platform data.
 */
#ifndef __LINUX_PLATFORM_DATA_CE5300_HDMI_H
#define __LINUX_PLATFORM_DATA_CE5300_HDMI_H

#include <linux/types.h>

struct device;
struct mutex;

#define CE5300_HDMI_AUDIO_NAME	"ce5300-hdmi-audio"

/* The sink, as far as audio cares. Written under the transmitter's lock. */
struct ce5300_hdmi_sink {
	bool	connected;
	bool	edid;		/* read and checksummed */
	bool	hdmi;		/* HDMI VSDB (OUI 00-0c-03); clear = DVI */
	bool	audio;		/* basic audio or an audio data block */
	char	name[14];
};

struct ce5300_hdmi_audio_pdata {
	void __iomem		*base;		/* the transmitter's BAR 0 */
	struct device		*dma_dev;	/* the transmitter's PCI device */
	struct mutex		*lock;		/* link setup vs. hotplug */
	const struct ce5300_hdmi_sink *sink;	/* read under @lock */
	struct device		*core;
	/*
	 * @fn runs after every sink change, from process context, with @lock
	 * held. Setting a NULL @fn (under @lock) stops it for good.
	 */
	void (*set_notify)(struct device *core, void (*fn)(void *priv), void *priv);
};

#endif
