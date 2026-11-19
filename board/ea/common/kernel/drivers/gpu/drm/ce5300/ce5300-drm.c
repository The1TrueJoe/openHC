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
 * GEM DMA buffers. Pipe A drives the HDMI transmitter, a DRM bridge
 * (ce5300-hdmi.c) found through the OF graph; its detect, EDID and hotplug
 * make the connector. Mode setting (the display PLL, the pipe and the HDMI
 * PHY) comes later; until then only the running mode is valid.
 *
 * Register access is limited to named registers. Unnamed holes in the VDC BAR
 * are not safe to read.
 */
#include <linux/aperture.h>
#include <linux/debugfs.h>
#include <linux/delay.h>
#include <linux/fs.h>
#include <linux/of.h>
#include <linux/of_graph.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/string.h>
#include <linux/uaccess.h>

#include <asm/iosf_mbi.h>

#include <drm/clients/drm_client_setup.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_bridge.h>
#include <drm/drm_bridge_connector.h>
#include <drm/drm_debugfs.h>
#include <drm/drm_drv.h>
#include <drm/drm_fb_dma_helper.h>
#include <drm/drm_fbdev_dma.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_atomic_helper.h>
#include <drm/drm_gem_dma_helper.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_managed.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_print.h>
#include <drm/drm_probe_helper.h>
#include <drm/drm_rect.h>
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
#define VDC_PIPEASRC		0x6001c	/* (hactive - 1) << 16 | (vactive - 1) */
#define VDC_PIPEACONF		0x70008
#define   VDC_PIPECONF_ENABLE	BIT(31)

/* UPP n at VDC_UPP + base (gdl_server's __init_upp_reg_table). */
#define VDC_UPP			0x30000
#define VDC_UPP_SURF		0x100
#define VDC_UPP_PITCH		0x118	/* bytes; read back 2944 under CEFDK */
#define VDC_UPP_POS		0x12c	/* y << 16 | x on the pipe */
#define VDC_UPP_DST_SIZE	0x130	/* height << 16 | width */
#define VDC_UPP_SRC_W		0x134
#define VDC_UPP_SRC_H		0x13c
#define VDC_UPP_CTRL		0x168
#define   UPP_CTRL_ENABLE	BIT(0)
#define   UPP_CTRL_PIPE_B	BIT(3)
#define   UPP_CTRL_FMT		GENMASK(14, 10)
#define   UPP_FMT_RGB_32	0x4800	/* GDL_PF_RGB_32 */
#define   UPP_FMT_ARGB_32	0x4c00	/* GDL_PF_ARGB_32: what CEFDK scans out */

static const u32 upp_bases[] = { 0x0000, 0x1000, 0x2000, 0x3000, 0xf000 };

#define PITCH_ALIGN		64

/*
 * Clocks, as CEFDK sets them at every boot (its gen5 splash_screen_gen5.c):
 * the dot clock is the display PLL, IOSF sideband port 0x8a, registers 0-7
 * and 0x11; the TMDS clock is the HDMI transmitter's PLL and PHY, port 0x82,
 * 23 registers. On both, writing 1 to register 0 latches the others (it
 * still reads back the table value). Their resets and the VDC clock gates are
 * in the clock-control unit (PCI 8086:2e52, BAR 0). PHY tables at 8 bpc.
 */
#define DPLL_PORT		0x8a
#define   DPLL_NREGS		8
#define   DPLL_LATCH		0	/* write 1 */
#define   DPLL_R1_RESET_HOLD	BIT(26)	/* Reset_in_b low while programming */
#define   DPLL_R1_DISABLE	0x06000000
#define   DPLL_REG_X11		0x11
#define   DPLL_X11		0x80
#define HDMI_PHY_PORT		0x82
#define   HDMI_PHY_NREGS	23
#define   HDMI_PHY_LATCH	0	/* write 1 */
#define   HDMI_PHY_POWER	12
#define     HDMI_PHY_GPD	BIT(16)			/* global power down */
#define     HDMI_PHY_APD	(BIT(2) | GENMASK(31, 29))	/* analog power down */

#define CCU_DEVICE		0x2e52
#define CCU_HDMI		0x04
#define   CCU_HDMI_PLL_DIV_RST	BIT(9)	/* 0 = held in reset */
#define   CCU_HDMI_TX_PLL_RST	BIT(7)
#define CCU_VDC			0x14
#define   CCU_VDC_CLKS		0x380	/* the gates CEFDK drops around a clock change */
#define CCU_DPLL		0x88

struct ce5300_clock {
	int khz;
	u32 dpll[DPLL_NREGS];
	u32 phy_pll[3];		/* PHY registers 0-2 */
	u32 phy_drive;		/* PHY registers 8-11 */
};

static const struct ce5300_clock ce5300_clocks[] = {
	{ 27000,
	  { 0x01802116, 0x0005002c, 0x045811a0, 0x4045800c,
	    0x04045800, 0x00404100, 0x02040440, 0x01424001 },
	  { 0x00105800, 0x00000a14, 0x00050005 }, 0xe8055053 },
	{ 74176,	/* 74.175 MHz: 720p and 1080i at 59.94 Hz */
	  { 0x01802116, 0x0005002c, 0x040411a0, 0x40404004,
	    0x04045800, 0x00004140, 0x92000404, 0x01424139 },
	  { 0x00002000, 0x00050028, 0x00090080 }, 0xe8055053 },
	{ 74250,
	  { 0x01802116, 0x0005002c, 0x044011a0, 0x80440008,
	    0x04045800, 0x00404100, 0x02040440, 0x01424001 },
	  { 0x00002000, 0x00050028, 0x00090080 }, 0xe80550f3 },
	{ 148352,	/* 148.35 MHz: 1080p at 59.94 Hz */
	  { 0x01802116, 0x0005002c, 0x040411a0, 0x40440000,
	    0x04045800, 0x000041c0, 0x12000404, 0x01424139 },
	  { 0x00002000, 0x00050028, 0x00050080 }, 0xe80550f3 },
};

/* PHY registers 3-7 and 12-22, the same for every clock. */
static const u32 ce5300_phy_3_7[] = { 0x00100000, 0x40214000, 0x80000001, 0x0007f03f, 0x02004000 };
static const u32 ce5300_phy_12_22[] = {
	0x1f010002, 0x00000000, 0x80000000, 0x36d8fff7, 0x00100000, 0xffd00000,
	0x00100000, 0x1c010000, 0x00000000, 0x00000000, 0x00000000,
};

static u32 ce5300_phy_word(const struct ce5300_clock *c, unsigned int i)
{
	if (i < 3)
		return c->phy_pll[i];
	if (i < 8)
		return ce5300_phy_3_7[i - 3];
	if (i < 12)
		return c->phy_drive;
	return ce5300_phy_12_22[i - 12];
}

/* Register 12 as it reads once the PHY is powered up. */
static u32 ce5300_phy_word_on(const struct ce5300_clock *c, unsigned int i)
{
	u32 v = ce5300_phy_word(c, i);

	return i == HDMI_PHY_POWER ? v & ~(HDMI_PHY_GPD | HDMI_PHY_APD) : v;
}

static const struct ce5300_clock *ce5300_clock_find(int khz)
{
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(ce5300_clocks); i++)
		if (abs(ce5300_clocks[i].khz - khz) <= ce5300_clocks[i].khz / 2000)
			return &ce5300_clocks[i];
	return NULL;
}

static bool takeover;
module_param(takeover, bool, 0644);
MODULE_PARM_DESC(takeover, "Take the display over from the firmware framebuffer (untested; set, then bind 01:08.0)");

static bool modeset;
module_param(modeset, bool, 0644);
MODULE_PARM_DESC(modeset, "Allow mode changes to the 480p, 720p and 1080p clocks (bring-up)");

struct ce5300 {
	struct drm_device	drm;
	void __iomem		*mmio;
	void __iomem		*ccu;		/* clock-control unit, or NULL */
	u32			upp;		/* the scanout UPP's register base */
	struct drm_display_mode	mode;		/* what the firmware runs */
	struct drm_display_mode	hw;		/* what pipe A runs now */

	struct drm_plane	plane;
	struct drm_crtc		crtc;
	struct drm_encoder	encoder;
	struct drm_bridge	*bridge;	/* the HDMI transmitter */
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
			drm_info(&p->drm, "firmware scanout: UPP %#x ctrl %#x surface %#x pitch %u\n",
				 p->upp, ctrl, vdc_read(p, p->upp + VDC_UPP_SURF),
				 vdc_read(p, p->upp + VDC_UPP_PITCH));
			return 0;
		}
	}
	return -ENODEV;
}

/* ---- Plane ------------------------------------------------------------- */

static const u32 ce5300_formats[] = {
	DRM_FORMAT_XRGB8888,
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
	ctrl |= UPP_FMT_RGB_32;

	vdc_write(p, p->upp + VDC_UPP_POS, ps->dst.y1 << 16 | ps->dst.x1);
	vdc_write(p, p->upp + VDC_UPP_DST_SIZE, drm_rect_height(&ps->dst) << 16 | drm_rect_width(&ps->dst));
	vdc_write(p, p->upp + VDC_UPP_SRC_W, drm_rect_width(&ps->src) >> 16);
	vdc_write(p, p->upp + VDC_UPP_SRC_H, drm_rect_height(&ps->src) >> 16);
	vdc_write(p, p->upp + VDC_UPP_PITCH, pitch);
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

static bool ce5300_mode_same(const struct drm_display_mode *a, const struct drm_display_mode *b)
{
	return a->hdisplay == b->hdisplay && a->vdisplay == b->vdisplay &&
	       a->htotal == b->htotal && a->vtotal == b->vtotal &&
	       a->hsync_start == b->hsync_start && a->hsync_end == b->hsync_end &&
	       a->vsync_start == b->vsync_start && a->vsync_end == b->vsync_end &&
	       abs(a->clock - b->clock) <= b->clock / 2000;	/* 59.94 isn't 60 */
}

static int ce5300_program_mode(struct ce5300 *p, const struct drm_display_mode *m);

static enum drm_mode_status ce5300_crtc_mode_valid(struct drm_crtc *crtc,
						   const struct drm_display_mode *m)
{
	struct ce5300 *p = to_ce5300(crtc->dev);

	if (ce5300_mode_same(m, &p->mode))
		return MODE_OK;
	if (!modeset)
		return MODE_BAD;
	if (m->flags & DRM_MODE_FLAG_INTERLACE)
		return MODE_NO_INTERLACE;
	if (m->hdisplay > 1920 || m->vdisplay > 1080)
		return MODE_BAD;
	return ce5300_clock_find(m->clock) ? MODE_OK : MODE_CLOCK_RANGE;
}

/* A new mode is programmed with the pipe off; the plane is already set up. */
static void ce5300_crtc_atomic_enable(struct drm_crtc *crtc, struct drm_atomic_commit *state)
{
	struct ce5300 *p = to_ce5300(crtc->dev);
	const struct drm_display_mode *m = &drm_atomic_get_new_crtc_state(state, crtc)->adjusted_mode;

	if (!ce5300_mode_same(m, &p->hw) && ce5300_program_mode(p, m))
		drm_err(&p->drm, "%s not set, pipe A stays at %s\n", m->name, p->hw.name);
	vdc_write(p, VDC_PIPEACONF, vdc_read(p, VDC_PIPEACONF) | VDC_PIPECONF_ENABLE);
}

static void ce5300_crtc_atomic_disable(struct drm_crtc *crtc, struct drm_atomic_commit *state)
{
	struct ce5300 *p = to_ce5300(crtc->dev);

	vdc_write(p, VDC_PIPEACONF, vdc_read(p, VDC_PIPEACONF) & ~VDC_PIPECONF_ENABLE);
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
	.atomic_enable	= ce5300_crtc_atomic_enable,
	.atomic_disable	= ce5300_crtc_atomic_disable,
};

static const struct drm_crtc_funcs ce5300_crtc_funcs = {
	.reset			= drm_atomic_helper_crtc_reset,
	.destroy		= drm_crtc_cleanup,
	.set_config		= drm_atomic_helper_set_config,
	.page_flip		= drm_atomic_helper_page_flip,
	.atomic_duplicate_state	= drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_crtc_destroy_state,
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

/* Pipe A's output, port 0 of the OF graph: the HDMI transmitter's bridge. */
static int ce5300_find_bridge(struct ce5300 *p)
{
	struct device_node *remote;

	remote = of_graph_get_remote_node(p->drm.dev->of_node, 0, 0);
	if (!remote)
		return dev_err_probe(p->drm.dev, -ENODEV, "no output in the device tree\n");
	p->bridge = of_drm_find_bridge(remote);
	of_node_put(remote);
	if (!p->bridge)
		return dev_err_probe(p->drm.dev, -EPROBE_DEFER, "HDMI bridge not up yet\n");
	return 0;
}

/* ---- Clocks: read-back, and CEFDK's programming sequence ------------- */

struct ce5300_clk_state {
	u32 dpll[DPLL_NREGS];
	u32 x11;
	u32 phy[HDMI_PHY_NREGS];
	u32 ccu_hdmi, ccu_vdc;
};

static int ce5300_sb_read(u8 port, u32 reg, u32 *val)
{
	return iosf_mbi_read(port, MBI_CR_READ, reg, val);
}

static int ce5300_sb_write(u8 port, u32 reg, u32 val)
{
	return iosf_mbi_write(port, MBI_CR_WRITE, reg, val);
}

static int ce5300_clk_read(struct ce5300 *p, struct ce5300_clk_state *st)
{
	unsigned int i;
	int ret = 0;

	for (i = 0; i < DPLL_NREGS && !ret; i++)
		ret = ce5300_sb_read(DPLL_PORT, i, &st->dpll[i]);
	if (!ret)
		ret = ce5300_sb_read(DPLL_PORT, DPLL_REG_X11, &st->x11);
	for (i = 0; i < HDMI_PHY_NREGS && !ret; i++)
		ret = ce5300_sb_read(HDMI_PHY_PORT, i, &st->phy[i]);
	st->ccu_hdmi = readl(p->ccu + CCU_HDMI);
	st->ccu_vdc = readl(p->ccu + CCU_VDC);
	return ret;
}

static void ce5300_clk_log(struct ce5300 *p, const char *what, const struct ce5300_clk_state *st)
{
	char buf[HDMI_PHY_NREGS * 9 + 1];
	unsigned int i, n = 0;

	for (i = 0; i < DPLL_NREGS; i++)
		n += scnprintf(buf + n, sizeof(buf) - n, " %08x", st->dpll[i]);
	drm_info(&p->drm, "%s: dpll%s r11 %08x ccu hdmi %08x vdc %08x\n", what, buf,
		 st->x11, st->ccu_hdmi, st->ccu_vdc);
	for (i = 0, n = 0; i < HDMI_PHY_NREGS; i++)
		n += scnprintf(buf + n, sizeof(buf) - n, " %08x", st->phy[i]);
	drm_info(&p->drm, "%s: phy%s\n", what, buf);
}

/* Does the hardware run @c, as CEFDK leaves it (PHY powered up)? */
static bool ce5300_clk_matches(struct ce5300 *p, const struct ce5300_clock *c,
			       const struct ce5300_clk_state *st)
{
	bool ok = st->x11 == DPLL_X11;
	unsigned int i;

	for (i = 0; i < DPLL_NREGS; i++)
		if (st->dpll[i] != c->dpll[i]) {
			drm_info(&p->drm, "dpll r%u %08x, %d kHz has %08x\n", i,
				 st->dpll[i], c->khz, c->dpll[i]);
			ok = false;
		}
	for (i = 0; i < HDMI_PHY_NREGS; i++)
		if (st->phy[i] != ce5300_phy_word_on(c, i)) {
			drm_info(&p->drm, "phy r%u %08x, %d kHz has %08x\n", i,
				 st->phy[i], c->khz, ce5300_phy_word_on(c, i));
			ok = false;
		}
	return ok;
}

/*
 * CEFDK's vdcClken: gate the VDC clocks, program the display PLL
 * (Clock_Config), then the HDMI transmitter's PLL and PHY
 * (program_clocks_bvl), ungate, and power the PHY up (phy_enable). The pipe
 * must be off. CEFDK then resets the whole HDMI unit, which would also reset
 * what ce5300-hdmi and its audio set up; that is left out until the bridge
 * can set it up again.
 */
static int ce5300_clk_write(struct ce5300 *p, const u32 *dpll, u32 x11, const u32 *phy)
{
	unsigned int i;
	int ret = 0;
	u32 v;

	writel(readl(p->ccu + CCU_VDC) & ~CCU_VDC_CLKS, p->ccu + CCU_VDC);

	ret |= ce5300_sb_write(DPLL_PORT, 1, DPLL_R1_DISABLE);
	ret |= ce5300_sb_write(DPLL_PORT, DPLL_LATCH, 1);
	for (i = 0; i < DPLL_NREGS; i++)
		ret |= ce5300_sb_write(DPLL_PORT, i, i == 1 ? dpll[i] | DPLL_R1_RESET_HOLD : dpll[i]);
	ret |= ce5300_sb_write(DPLL_PORT, DPLL_LATCH, 1);
	udelay(300);
	for (i = 0; i < DPLL_NREGS; i++)
		ret |= ce5300_sb_write(DPLL_PORT, i, i == 1 ? dpll[i] & ~DPLL_R1_RESET_HOLD : dpll[i]);
	ret |= ce5300_sb_write(DPLL_PORT, DPLL_REG_X11, x11);
	ret |= ce5300_sb_write(DPLL_PORT, DPLL_LATCH, 1);

	v = readl(p->ccu + CCU_HDMI) & ~CCU_HDMI_PLL_DIV_RST;
	writel(v, p->ccu + CCU_HDMI);
	v &= ~CCU_HDMI_TX_PLL_RST;
	writel(v, p->ccu + CCU_HDMI);
	for (i = 0; i < HDMI_PHY_NREGS; i++)
		ret |= ce5300_sb_write(HDMI_PHY_PORT, i, phy[i]);
	ret |= ce5300_sb_write(HDMI_PHY_PORT, HDMI_PHY_POWER,
			       (phy[HDMI_PHY_POWER] & ~HDMI_PHY_GPD) | HDMI_PHY_APD);
	ret |= ce5300_sb_write(HDMI_PHY_PORT, HDMI_PHY_LATCH, 1);
	v |= CCU_HDMI_TX_PLL_RST;
	writel(v, p->ccu + CCU_HDMI);
	udelay(300);
	v |= CCU_HDMI_PLL_DIV_RST;
	writel(v, p->ccu + CCU_HDMI);
	udelay(50);

	writel(readl(p->ccu + CCU_VDC) | CCU_VDC_CLKS, p->ccu + CCU_VDC);

	ret |= ce5300_sb_read(HDMI_PHY_PORT, HDMI_PHY_POWER, &v);
	ret |= ce5300_sb_write(HDMI_PHY_PORT, HDMI_PHY_POWER,
			       v & ~(HDMI_PHY_GPD | HDMI_PHY_APD));
	return ret ? -EIO : 0;
}

static int ce5300_clocks_show(struct seq_file *m, void *unused)
{
	struct drm_debugfs_entry *entry = m->private;
	struct ce5300 *p = to_ce5300(entry->dev);
	struct ce5300_clk_state st;
	unsigned int i, j;
	int ret;

	if (!iosf_mbi_available() || !p->ccu) {
		seq_puts(m, "no IOSF mailbox or clock-control unit\n");
		return 0;
	}
	ret = ce5300_clk_read(p, &st);
	if (ret)
		return ret;

	seq_puts(m, "dpll");
	for (i = 0; i < DPLL_NREGS; i++)
		seq_printf(m, " %08x", st.dpll[i]);
	seq_printf(m, " r11 %08x\nphy", st.x11);
	for (i = 0; i < HDMI_PHY_NREGS; i++)
		seq_printf(m, " %08x", st.phy[i]);
	seq_printf(m, "\nccu hdmi %08x vdc %08x dpll %08x\n", st.ccu_hdmi, st.ccu_vdc,
		   readl(p->ccu + CCU_DPLL));

	for (j = 0; j < ARRAY_SIZE(ce5300_clocks); j++) {
		const struct ce5300_clock *c = &ce5300_clocks[j];

		seq_printf(m, "%6d kHz: dpll differs in", c->khz);
		for (i = 0; i < DPLL_NREGS; i++)
			if (st.dpll[i] != c->dpll[i])
				seq_printf(m, " r%u", i);
		seq_puts(m, "; phy differs in");
		for (i = 0; i < HDMI_PHY_NREGS; i++)
			if (st.phy[i] != ce5300_phy_word_on(c, i))
				seq_printf(m, " r%u", i);
		seq_putc(m, '\n');
	}
	return 0;
}

/* ---- Mode setting ---------------------------------------------------- */

/* Pipe A timing words, then PIPESRC. */
static const u32 ce5300_pipe_regs[] = {
	VDC_HTOTAL_A, VDC_HBLANK_A, VDC_HSYNC_A, VDC_VTOTAL_A, VDC_VBLANK_A, VDC_VSYNC_A,
	VDC_PIPEASRC,
};

#define VDC_SPAN(end, start)	(((end) - 1) << 16 | ((start) - 1))

static void ce5300_mode_timings(const struct drm_display_mode *m, u32 *t)
{
	t[0] = VDC_SPAN(m->htotal, m->hdisplay);
	t[1] = VDC_SPAN(m->htotal, m->hdisplay);	/* blanking: active to total */
	t[2] = VDC_SPAN(m->hsync_end, m->hsync_start);
	t[3] = VDC_SPAN(m->vtotal, m->vdisplay);
	t[4] = VDC_SPAN(m->vtotal, m->vdisplay);
	t[5] = VDC_SPAN(m->vsync_end, m->vsync_start);
	t[6] = VDC_SPAN(m->hdisplay, m->vdisplay);
}

static void ce5300_pipe_write(struct ce5300 *p, const struct ce5300_clock *c, const u32 *dpll,
			      u32 x11, const u32 *phy, const u32 *timing)
{
	unsigned int i;

	if (ce5300_clk_write(p, dpll, x11, phy))
		drm_err(&p->drm, "%d kHz: a sideband write failed\n", c->khz);
	for (i = 0; i < ARRAY_SIZE(ce5300_pipe_regs); i++)
		vdc_write(p, ce5300_pipe_regs[i], timing[i]);
}

/*
 * Program @m on pipe A with CEFDK's sequence. Every register written is read
 * and logged first, and nothing is written unless the clocks are the table of
 * the mode running now. If a read-back afterwards differs, what was read
 * first goes back in. The pipe and the plane come back as they were.
 */
static int ce5300_program_mode(struct ce5300 *p, const struct drm_display_mode *m)
{
	const struct ce5300_clock *c = ce5300_clock_find(m->clock);
	const struct ce5300_clock *cur = ce5300_clock_find(p->hw.clock);
	u32 timing[ARRAY_SIZE(ce5300_pipe_regs)], old[ARRAY_SIZE(ce5300_pipe_regs)];
	u32 phy[HDMI_PHY_NREGS], pipeconf, upp_ctrl;
	struct ce5300_clk_state before, after;
	unsigned int i;
	bool ok;
	int ret;

	if (!c || !cur || !p->ccu || !iosf_mbi_available())
		return -ENODEV;
	ret = ce5300_clk_read(p, &before);
	if (ret)
		return ret;
	ce5300_clk_log(p, "modeset: before", &before);
	if (!ce5300_clk_matches(p, cur, &before)) {
		drm_err(&p->drm, "modeset: the clocks aren't the %d kHz table; nothing written\n",
			cur->khz);
		return -EINVAL;
	}
	for (i = 0; i < ARRAY_SIZE(ce5300_pipe_regs); i++)
		old[i] = vdc_read(p, ce5300_pipe_regs[i]);
	pipeconf = vdc_read(p, VDC_PIPEACONF);
	upp_ctrl = vdc_read(p, p->upp + VDC_UPP_CTRL);
	drm_info(&p->drm, "modeset: pipe %08x %08x %08x %08x %08x %08x src %08x conf %08x upp %08x\n",
		 old[0], old[1], old[2], old[3], old[4], old[5], old[6], pipeconf, upp_ctrl);
	ce5300_mode_timings(m, timing);
	for (i = 0; i < HDMI_PHY_NREGS; i++)
		phy[i] = ce5300_phy_word(c, i);

	vdc_write(p, p->upp + VDC_UPP_CTRL, upp_ctrl & ~UPP_CTRL_ENABLE);
	vdc_write(p, VDC_PIPEACONF, pipeconf & ~VDC_PIPECONF_ENABLE);
	msleep(50);	/* two frames at 480p */
	ce5300_pipe_write(p, c, c->dpll, DPLL_X11, phy, timing);

	ok = !ce5300_clk_read(p, &after);
	if (ok) {
		ce5300_clk_log(p, "modeset: after", &after);
		ok = ce5300_clk_matches(p, c, &after);
	}
	for (i = 0; i < ARRAY_SIZE(ce5300_pipe_regs); i++)
		ok &= vdc_read(p, ce5300_pipe_regs[i]) == timing[i];
	if (!ok) {
		drm_err(&p->drm, "modeset: read-back differs; putting the first read back\n");
		ce5300_pipe_write(p, cur, before.dpll, before.x11, before.phy, old);
		if (!ce5300_clk_read(p, &after))
			ce5300_clk_log(p, "modeset: restored", &after);
	}
	vdc_write(p, VDC_PIPEACONF, pipeconf);
	vdc_write(p, p->upp + VDC_UPP_CTRL, upp_ctrl);
	if (!ok)
		return -EIO;
	drm_mode_copy(&p->hw, m);
	drm_info(&p->drm, "modeset: %s at %d kHz, read-back matches\n", m->name, c->khz);
	return 0;
}

/* Bring-up: the running mode programmed again, unchanged. */
static int ce5300_noop_modeset(struct ce5300 *p)
{
	struct drm_display_mode m;
	int ret;

	drm_modeset_lock_all(&p->drm);
	drm_mode_copy(&m, &p->hw);
	ret = ce5300_program_mode(p, &m);
	drm_modeset_unlock_all(&p->drm);
	return ret;
}

static ssize_t ce5300_modeset_write(struct file *f, const char __user *ubuf,
				    size_t len, loff_t *ppos)
{
	struct ce5300 *p = f->private_data;
	char cmd[8] = { };
	int ret;

	if (len >= sizeof(cmd) || copy_from_user(cmd, ubuf, len))
		return -EINVAL;
	if (!sysfs_streq(cmd, "noop"))
		return -EINVAL;
	ret = ce5300_noop_modeset(p);
	return ret ? ret : len;
}

static const struct file_operations ce5300_modeset_fops = {
	.owner	= THIS_MODULE,
	.open	= simple_open,
	.write	= ce5300_modeset_write,
};

/* The clock-control unit, read-only here: one register per block. */
static void ce5300_map_ccu(struct ce5300 *p)
{
	struct pci_dev *ccu = pci_get_device(PCI_VENDOR_ID_INTEL, CCU_DEVICE, NULL);

	if (!ccu)
		return;
	if (pci_resource_len(ccu, 0) > CCU_DPLL)
		p->ccu = devm_ioremap(p->drm.dev, pci_resource_start(ccu, 0),
				      pci_resource_len(ccu, 0));
	pci_dev_put(ccu);
}

static int ce5300_modeset_init(struct ce5300 *p)
{
	struct drm_device *drm = &p->drm;
	struct drm_connector *connector;
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

	ret = drm_bridge_attach(&p->encoder, p->bridge, NULL, DRM_BRIDGE_ATTACH_NO_CONNECTOR);
	if (ret)
		return ret;
	connector = drm_bridge_connector_init(drm, &p->encoder);
	if (IS_ERR(connector))
		return PTR_ERR(connector);
	ret = drm_connector_attach_encoder(connector, &p->encoder);
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
	drm_mode_copy(&p->hw, &p->mode);
	if (ret) {
		drm_err(&p->drm, "pipe A is off or unreadable; nothing to take over\n");
		return ret;
	}
	ret = ce5300_find_plane(p);
	if (ret) {
		drm_err(&p->drm, "no enabled UPP on pipe A\n");
		return ret;
	}
	ret = ce5300_find_bridge(p);
	if (ret)
		return ret;
	ce5300_map_ccu(p);

	/* The firmware framebuffer lives in RAM, not in a BAR. */
	surf = vdc_read(p, p->upp + VDC_UPP_SURF);
	ret = aperture_remove_conflicting_devices(surf,
						  p->mode.vdisplay * vdc_read(p, p->upp + VDC_UPP_PITCH),
						  DRIVER_NAME);
	if (ret)
		return ret;

	ret = ce5300_modeset_init(p);
	if (ret)
		return ret;
	pci_set_drvdata(pdev, &p->drm);

	drm_debugfs_add_file(&p->drm, "ce5300_clocks", ce5300_clocks_show, NULL);
	ret = drm_dev_register(&p->drm, 0);
	if (ret)
		return ret;
	drm_client_setup(&p->drm, NULL);
	debugfs_create_file("ce5300_modeset", 0200, p->drm.debugfs_root, p, &ce5300_modeset_fops);
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
