// SPDX-License-Identifier: GPL-2.0-only
/*
 * KMS driver for the Intel Atom CE5300 display controller (VDC, PCI 8086:2e61)
 * and its HDMI output.
 *
 * The VDC is an i9xx-style display pipe with its own plane blocks:
 *  - five "universal pixel planes" (UPP, any RGB/YUV format) at 0x30000 + base;
 *  - three 8-bit indexed/alpha planes (IAP) at 0x10000 + base;
 *  - a cursor at 0x40000.
 * The firmware scans its console out of one UPP on pipe A.
 *
 * This first step takes over the mode the firmware set and keeps it. It reads
 * pipe A and the live UPP back, and from then on flips that plane between
 * GEM DMA buffers. Hotplug is polled through the HDMI transmitter's HPD bit,
 * and the EDID comes over the transmitter's DDC master, which ce5300-hdmi
 * registers as an i2c adapter. Mode setting (the display PLL, the pipe and the
 * HDMI PHY) comes later; until then only the running mode is valid.
 *
 * Register access is limited to named registers. Unnamed holes in the VDC BAR
 * are not safe to read.
 */
#include <linux/aperture.h>
#include <linux/i2c.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/pci.h>

#include <drm/clients/drm_client_setup.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_drv.h>
#include <drm/drm_edid.h>
#include <drm/drm_fb_dma_helper.h>
#include <drm/drm_fbdev_dma.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_atomic_helper.h>
#include <drm/drm_gem_dma_helper.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_managed.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_print.h>
#include <drm/drm_probe_helper.h>
#include <drm/drm_simple_kms_helper.h>
#include <drm/drm_vblank.h>

#define DRIVER_NAME		"ce5300"
#define DRIVER_DESC		"Intel CE5300 display"

/* Pipe A timing, i9xx layout: (end - 1) << 16 | (start - 1). */
#define VDC_HTOTAL_A		0x60000
#define VDC_HBLANK_A		0x60004
#define VDC_HSYNC_A		0x60008
#define VDC_VTOTAL_A		0x6000c
#define VDC_VBLANK_A		0x60010
#define VDC_VSYNC_A		0x60014
#define VDC_PIPEACONF		0x70008
#define   VDC_PIPECONF_ENABLE	BIT(31)

/* UPP n at VDC_UPP + base (gdl_server's __init_upp_reg_table). */
#define VDC_UPP			0x30000
#define VDC_UPP_PITCH		0x064	/* Y pitch << 16 | UV pitch */
#define VDC_UPP_SURF		0x100
#define VDC_UPP_SIZE		0x118	/* (h - 1)? << 16 | (w - 1)?: read back */
#define VDC_UPP_CTRL		0x168
#define   UPP_CTRL_ENABLE	BIT(0)
#define   UPP_CTRL_PIPE_B	BIT(3)
#define   UPP_CTRL_FMT		GENMASK(14, 10)
#define   UPP_FMT_RGB_32	0x4800	/* GDL_PF_RGB_32 */
#define   UPP_FMT_ARGB_32	0x4c00	/* GDL_PF_ARGB_32 */

static const u32 upp_bases[] = { 0x0000, 0x1000, 0x2000, 0x3000, 0xf000 };

/* The HDMI transmitter (PCI 8086:2e63): HPD only, read-only. */
#define HDMI_TX_DEVICE		0x2e63
#define HDMI_TX_STATUS		0x1008
#define   HDMI_TX_HPD		BIT(0)
#define HDMI_DDC_NAME		"ce5300-hdmi DDC"

#define PITCH_ALIGN		64

static bool takeover;
module_param(takeover, bool, 0444);
MODULE_PARM_DESC(takeover, "Take the display over from the firmware framebuffer (untested)");

struct ce5300 {
	struct drm_device	drm;
	void __iomem		*mmio;
	void __iomem		*hdmi_status;	/* one register of the TX */
	u32			upp;		/* the scanout UPP's register base */
	struct drm_display_mode	mode;		/* what the firmware runs */

	struct drm_plane	plane;
	struct drm_crtc		crtc;
	struct drm_encoder	encoder;
	struct drm_connector	connector;
};

#define to_ce5300(d) container_of(d, struct ce5300, drm)

static u32 vdc_read(struct ce5300 *p, u32 reg)
{
	return readl(p->mmio + reg);
}

static void vdc_write(struct ce5300 *p, u32 reg, u32 val)
{
	writel(val, p->mmio + reg);
}

/* ---- What the firmware left -------------------------------------------- */

static int ce5300_read_mode(struct ce5300 *p)
{
	struct drm_display_mode *m = &p->mode;
	u32 ht = vdc_read(p, VDC_HTOTAL_A), hs = vdc_read(p, VDC_HSYNC_A);
	u32 vt = vdc_read(p, VDC_VTOTAL_A), vs = vdc_read(p, VDC_VSYNC_A);

	if (!(vdc_read(p, VDC_PIPEACONF) & VDC_PIPECONF_ENABLE) || !ht || !vt)
		return -ENODEV;

	m->hdisplay = (ht & 0xfff) + 1;
	m->htotal = (ht >> 16 & 0xfff) + 1;
	m->hsync_start = (hs & 0xfff) + 1;
	m->hsync_end = (hs >> 16 & 0xfff) + 1;
	m->vdisplay = (vt & 0xfff) + 1;
	m->vtotal = (vt >> 16 & 0xfff) + 1;
	m->vsync_start = (vs & 0xfff) + 1;
	m->vsync_end = (vs >> 16 & 0xfff) + 1;
	/* The dot clock is the display PLL's, not read back yet: CEA 480p. */
	if (m->htotal == 858 && m->vtotal == 525) {
		m->clock = 27000;
		m->flags = DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_NVSYNC;
	} else {
		m->clock = DIV_ROUND_UP(m->htotal * m->vtotal * 60, 1000);
		drm_warn(&p->drm, "unknown firmware timing %ux%u, assuming 60 Hz\n",
			 m->htotal, m->vtotal);
	}
	m->type = DRM_MODE_TYPE_DRIVER | DRM_MODE_TYPE_PREFERRED;
	drm_mode_set_name(m);
	return 0;
}

static int ce5300_find_plane(struct ce5300 *p)
{
	unsigned int i;
	u32 ctrl;

	for (i = 0; i < ARRAY_SIZE(upp_bases); i++) {
		ctrl = vdc_read(p, VDC_UPP + upp_bases[i] + VDC_UPP_CTRL);
		if ((ctrl & UPP_CTRL_ENABLE) && !(ctrl & UPP_CTRL_PIPE_B)) {
			p->upp = VDC_UPP + upp_bases[i];
			drm_info(&p->drm, "firmware scanout: UPP %#x ctrl %#x surface %#x pitch %#x size %#x\n",
				 p->upp, ctrl, vdc_read(p, p->upp + VDC_UPP_SURF),
				 vdc_read(p, p->upp + VDC_UPP_PITCH),
				 vdc_read(p, p->upp + VDC_UPP_SIZE));
			return 0;
		}
	}
	return -ENODEV;
}

/* ---- Plane ------------------------------------------------------------- */

static const u32 ce5300_formats[] = {
	DRM_FORMAT_XRGB8888,
	DRM_FORMAT_ARGB8888,
};

static int ce5300_plane_atomic_check(struct drm_plane *plane, struct drm_atomic_commit *state)
{
	struct drm_plane_state *ps = drm_atomic_get_new_plane_state(state, plane);
	struct drm_crtc_state *cs = NULL;

	if (ps->crtc)
		cs = drm_atomic_get_new_crtc_state(state, ps->crtc);
	return drm_atomic_helper_check_plane_state(ps, cs, DRM_PLANE_NO_SCALING,
						   DRM_PLANE_NO_SCALING, false, false);
}

static void ce5300_plane_atomic_update(struct drm_plane *plane, struct drm_atomic_commit *state)
{
	struct ce5300 *p = to_ce5300(plane->dev);
	struct drm_plane_state *ps = drm_atomic_get_new_plane_state(state, plane);
	struct drm_framebuffer *fb = ps->fb;
	u32 ctrl, pitch;

	if (!fb || !ps->visible)
		return;
	pitch = fb->pitches[0];
	ctrl = vdc_read(p, p->upp + VDC_UPP_CTRL) & ~UPP_CTRL_FMT;
	ctrl |= fb->format->format == DRM_FORMAT_ARGB8888 ? UPP_FMT_ARGB_32 : UPP_FMT_RGB_32;

	vdc_write(p, p->upp + VDC_UPP_PITCH, pitch << 16 | pitch);
	vdc_write(p, p->upp + VDC_UPP_SURF, drm_fb_dma_get_gem_addr(fb, ps, 0));
	vdc_write(p, p->upp + VDC_UPP_CTRL, ctrl | UPP_CTRL_ENABLE);
}

static const struct drm_plane_helper_funcs ce5300_plane_helper_funcs = {
	.atomic_check	= ce5300_plane_atomic_check,
	.atomic_update	= ce5300_plane_atomic_update,
};

static const struct drm_plane_funcs ce5300_plane_funcs = {
	.update_plane		= drm_atomic_helper_update_plane,
	.disable_plane		= drm_atomic_helper_disable_plane,
	.destroy		= drm_plane_cleanup,
	.reset			= drm_atomic_helper_plane_reset,
	.atomic_duplicate_state	= drm_atomic_helper_plane_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_plane_destroy_state,
};

/* ---- CRTC: pipe A, as the firmware runs it ----------------------------- */

static bool ce5300_mode_is_current(struct ce5300 *p, const struct drm_display_mode *m)
{
	return m->hdisplay == p->mode.hdisplay && m->vdisplay == p->mode.vdisplay &&
	       m->htotal == p->mode.htotal && m->vtotal == p->mode.vtotal &&
	       abs(m->clock - p->mode.clock) <= p->mode.clock / 500;
}

static enum drm_mode_status ce5300_crtc_mode_valid(struct drm_crtc *crtc,
						   const struct drm_display_mode *m)
{
	return ce5300_mode_is_current(to_ce5300(crtc->dev), m) ? MODE_OK : MODE_BAD;
}

static int ce5300_crtc_atomic_check(struct drm_crtc *crtc, struct drm_atomic_commit *state)
{
	struct drm_crtc_state *cs = drm_atomic_get_new_crtc_state(state, crtc);

	return drm_atomic_helper_check_crtc_primary_plane(cs);
}

/* No vblank interrupt yet: complete the event at once. */
static void ce5300_crtc_atomic_flush(struct drm_crtc *crtc, struct drm_atomic_commit *state)
{
	struct drm_crtc_state *cs = drm_atomic_get_new_crtc_state(state, crtc);
	struct drm_pending_vblank_event *event = cs->event;

	if (!event)
		return;
	cs->event = NULL;
	spin_lock_irq(&crtc->dev->event_lock);
	drm_crtc_send_vblank_event(crtc, event);
	spin_unlock_irq(&crtc->dev->event_lock);
}

static const struct drm_crtc_helper_funcs ce5300_crtc_helper_funcs = {
	.mode_valid	= ce5300_crtc_mode_valid,
	.atomic_check	= ce5300_crtc_atomic_check,
	.atomic_flush	= ce5300_crtc_atomic_flush,
};

static const struct drm_crtc_funcs ce5300_crtc_funcs = {
	.reset			= drm_atomic_helper_crtc_reset,
	.destroy		= drm_crtc_cleanup,
	.set_config		= drm_atomic_helper_set_config,
	.page_flip		= drm_atomic_helper_page_flip,
	.atomic_duplicate_state	= drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_crtc_destroy_state,
};

/* ---- HDMI connector ---------------------------------------------------- */

static int ce5300_find_ddc(struct device *dev, void *data)
{
	struct i2c_adapter *adap = i2c_verify_adapter(dev);
	struct i2c_adapter **found = data;

	if (adap && !strcmp(adap->name, HDMI_DDC_NAME)) {
		*found = i2c_get_adapter(adap->nr);
		return 1;
	}
	return 0;
}

static enum drm_connector_status ce5300_connector_detect(struct drm_connector *c, bool force)
{
	struct ce5300 *p = to_ce5300(c->dev);

	if (!p->hdmi_status)
		return connector_status_unknown;
	return readl(p->hdmi_status) & HDMI_TX_HPD ? connector_status_connected :
						      connector_status_disconnected;
}

static int ce5300_connector_get_modes(struct drm_connector *c)
{
	struct ce5300 *p = to_ce5300(c->dev);
	struct i2c_adapter *ddc = NULL;
	const struct drm_edid *edid = NULL;
	struct drm_display_mode *m;
	int n = 0;

	i2c_for_each_dev(&ddc, ce5300_find_ddc);
	if (ddc) {
		edid = drm_edid_read_ddc(c, ddc);
		i2c_put_adapter(ddc);
	}
	drm_edid_connector_update(c, edid);
	if (edid)
		n = drm_edid_connector_add_modes(c);
	drm_edid_free(edid);

	/* Until there is a modeset, the running mode is the one to offer. */
	m = drm_mode_duplicate(c->dev, &p->mode);
	if (m) {
		drm_mode_probed_add(c, m);
		n++;
	}
	return n;
}

static enum drm_mode_status ce5300_connector_mode_valid(struct drm_connector *c,
							const struct drm_display_mode *m)
{
	return ce5300_mode_is_current(to_ce5300(c->dev), m) ? MODE_OK : MODE_BAD;
}

static const struct drm_connector_helper_funcs ce5300_connector_helper_funcs = {
	.get_modes	= ce5300_connector_get_modes,
	.mode_valid	= ce5300_connector_mode_valid,
};

static const struct drm_connector_funcs ce5300_connector_funcs = {
	.detect			= ce5300_connector_detect,
	.fill_modes		= drm_helper_probe_single_connector_modes,
	.destroy		= drm_connector_cleanup,
	.reset			= drm_atomic_helper_connector_reset,
	.atomic_duplicate_state	= drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_connector_destroy_state,
};

/* ---- Device ------------------------------------------------------------ */

static int ce5300_dumb_create(struct drm_file *file, struct drm_device *drm,
			      struct drm_mode_create_dumb *args)
{
	args->pitch = ALIGN(DIV_ROUND_UP(args->width * args->bpp, 8), PITCH_ALIGN);
	return drm_gem_dma_dumb_create_internal(file, drm, args);
}

static const struct drm_mode_config_funcs ce5300_mode_config_funcs = {
	.fb_create	= drm_gem_fb_create,
	.atomic_check	= drm_atomic_helper_check,
	.atomic_commit	= drm_atomic_helper_commit,
};

DEFINE_DRM_GEM_DMA_FOPS(ce5300_fops);

static const struct drm_driver ce5300_driver = {
	.driver_features	= DRIVER_MODESET | DRIVER_GEM | DRIVER_ATOMIC,
	.fops			= &ce5300_fops,
	DRM_GEM_DMA_DRIVER_OPS_WITH_DUMB_CREATE(ce5300_dumb_create),
	DRM_FBDEV_DMA_DRIVER_OPS,
	.name			= DRIVER_NAME,
	.desc			= DRIVER_DESC,
	.major			= 0,
	.minor			= 1,
};

/* The HPD bit of the transmitter, mapped read-only: ce5300-hdmi owns the BAR. */
static void ce5300_map_hpd(struct ce5300 *p)
{
	struct pci_dev *tx = pci_get_device(PCI_VENDOR_ID_INTEL, HDMI_TX_DEVICE, NULL);

	if (!tx)
		return;
	p->hdmi_status = devm_ioremap(p->drm.dev, pci_resource_start(tx, 0) + HDMI_TX_STATUS, 4);
	pci_dev_put(tx);
}

static int ce5300_modeset_init(struct ce5300 *p)
{
	struct drm_device *drm = &p->drm;
	int ret;

	ret = drmm_mode_config_init(drm);
	if (ret)
		return ret;
	drm->mode_config.min_width = 64;
	drm->mode_config.min_height = 64;
	drm->mode_config.max_width = 1920;
	drm->mode_config.max_height = 1080;
	drm->mode_config.preferred_depth = 24;
	drm->mode_config.funcs = &ce5300_mode_config_funcs;

	ret = drm_universal_plane_init(drm, &p->plane, 0, &ce5300_plane_funcs,
				       ce5300_formats, ARRAY_SIZE(ce5300_formats), NULL,
				       DRM_PLANE_TYPE_PRIMARY, NULL);
	if (ret)
		return ret;
	drm_plane_helper_add(&p->plane, &ce5300_plane_helper_funcs);

	ret = drm_crtc_init_with_planes(drm, &p->crtc, &p->plane, NULL,
					&ce5300_crtc_funcs, NULL);
	if (ret)
		return ret;
	drm_crtc_helper_add(&p->crtc, &ce5300_crtc_helper_funcs);

	ret = drm_simple_encoder_init(drm, &p->encoder, DRM_MODE_ENCODER_TMDS);
	if (ret)
		return ret;
	p->encoder.possible_crtcs = drm_crtc_mask(&p->crtc);

	ret = drm_connector_init(drm, &p->connector, &ce5300_connector_funcs,
				 DRM_MODE_CONNECTOR_HDMIA);
	if (ret)
		return ret;
	drm_connector_helper_add(&p->connector, &ce5300_connector_helper_funcs);
	p->connector.polled = DRM_CONNECTOR_POLL_CONNECT | DRM_CONNECTOR_POLL_DISCONNECT;
	ret = drm_connector_attach_encoder(&p->connector, &p->encoder);
	if (ret)
		return ret;

	drm_mode_config_reset(drm);
	drmm_kms_helper_poll_init(drm);
	return 0;
}

static int ce5300_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct ce5300 *p;
	u32 surf;
	int ret;

	if (!takeover)
		return -ENODEV;

	p = devm_drm_dev_alloc(&pdev->dev, &ce5300_driver, struct ce5300, drm);
	if (IS_ERR(p))
		return PTR_ERR(p);

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	p->mmio = pcim_iomap(pdev, 0, 0);
	if (!p->mmio)
		return -ENOMEM;
	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;

	ret = ce5300_read_mode(p);
	if (ret) {
		drm_err(&p->drm, "pipe A is off or unreadable; nothing to take over\n");
		return ret;
	}
	ret = ce5300_find_plane(p);
	if (ret) {
		drm_err(&p->drm, "no enabled UPP on pipe A\n");
		return ret;
	}
	ce5300_map_hpd(p);

	/* The firmware framebuffer lives in RAM, not in a BAR. */
	surf = vdc_read(p, p->upp + VDC_UPP_SURF);
	ret = aperture_remove_conflicting_devices(surf,
						  p->mode.vdisplay * (vdc_read(p, p->upp + VDC_UPP_PITCH) >> 16),
						  DRIVER_NAME);
	if (ret)
		return ret;

	ret = ce5300_modeset_init(p);
	if (ret)
		return ret;
	pci_set_drvdata(pdev, &p->drm);

	ret = drm_dev_register(&p->drm, 0);
	if (ret)
		return ret;
	drm_client_setup(&p->drm, NULL);
	drm_info(&p->drm, "taking over %s from the firmware\n", p->mode.name);
	return 0;
}

static void ce5300_remove(struct pci_dev *pdev)
{
	struct drm_device *drm = pci_get_drvdata(pdev);

	drm_dev_unplug(drm);
	drm_atomic_helper_shutdown(drm);
}

static void ce5300_shutdown(struct pci_dev *pdev)
{
	struct drm_device *drm = pci_get_drvdata(pdev);

	if (drm)
		drm_atomic_helper_shutdown(drm);
}

static const struct pci_device_id ce5300_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x2e61) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce5300_ids);

static struct pci_driver ce5300_pci_driver = {
	.name		= DRIVER_NAME,
	.id_table	= ce5300_ids,
	.probe		= ce5300_probe,
	.remove		= ce5300_remove,
	.shutdown	= ce5300_shutdown,
};
module_pci_driver(ce5300_pci_driver);

MODULE_DESCRIPTION(DRIVER_DESC);
MODULE_LICENSE("GPL");
