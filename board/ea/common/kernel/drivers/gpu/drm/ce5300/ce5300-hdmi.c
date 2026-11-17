// SPDX-License-Identifier: GPL-2.0-only
/*
 * DRM bridge driver for the Intel CE5300 HDMI transmitter (PCI 8086:2e63).
 *
 * The transmitter sits after the display controller's pipe A (ce5300-drm). It
 * owns the BAR; this driver does what every user of it shares:
 *  - the connector's +5V: +0x1000 bit 3 and, on revision 6, PHY register 12
 *    bit 17 clear (stock's hdmi_general_5V_enable). A sink raises HPD from it;
 *  - its own DDC master, the only I2C path to the connector, as an i2c adapter;
 *  - the plug state, polled: HPD (+0x1008 bit 0, plus its latched change bit),
 *    or else an EDID that answers, since on an EA1 HPD stays low with a monitor
 *    in. Changes go to DRM (hotplug) and to the audio driver;
 *  - the sink's HDMI and audio support from its EDID.
 * Audio (the transmitter's own DMA engine) is a child platform device driven
 * by sound/x86/ce5300-hdmi-audio.c, as dw-hdmi does with its AHB audio.
 */
#include <linux/debugfs.h>
#include <linux/delay.h>
#include <linux/i2c.h>
#include <linux/io.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/pci.h>
#include <linux/platform_data/ce5300-hdmi.h>
#include <linux/platform_device.h>
#include <linux/seq_file.h>
#include <linux/slab.h>
#include <linux/unaligned.h>
#include <linux/workqueue.h>

#include <asm/iosf_mbi.h>

#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_bridge.h>
#include <drm/drm_edid.h>

#define DRV_NAME		"ce5300-hdmi"

#define HDMI_CLOCKS		0x1000
#define   HDMI_5V_ENABLE	BIT(3)	/* hdmi_general_5V_enable */
#define HDMI_INT_MASK		0x1004
#define   HDMI_INT_MASK_STOCK	0x1ae0bf	/* __hdmi_hw_init's value */
#define HDMI_STATUS		0x1008	/* hdmi_general_get_status */
#define   HDMI_STATUS_HPD	BIT(0)
/* Latched whether masked or not; write 1 to clear. */
#define HDMI_INT_STATUS		0x100c
#define   HDMI_INT_HPD		BIT(0)	/* HPD changed */
#define   HDMI_INT_DDC_DONE	BIT(2)
#define   HDMI_INT_DDC_FULL	BIT(3)	/* 64 bytes in the buffer */
#define   HDMI_INT_DDC_ERROR	BIT(4)	/* NAK, arbitration */
#define   HDMI_INT_DDC		(HDMI_INT_DDC_DONE | HDMI_INT_DDC_FULL | HDMI_INT_DDC_ERROR)
#define HDMI_OPMODE		0x3000
#define   HDMI_OPMODE_HDMI	BIT(0)	/* clear = DVI */

/* DDC master (pd_hdmi's hdmi_i2c_*; CE5300 = its generic read) */
#define HDMI_DDC_CTRL		0x1100
#define   HDMI_DDC_RESET	BIT(14)
#define   HDMI_DDC_ON		0x8760	/* hdmi_i2c_enable(.., 1) */
#define HDMI_DDC_STATUS		0x1104
#define   HDMI_DDC_BUSY		(BIT(2) | BIT(3))	/* transaction_start refuses */
#define HDMI_DDC_LINES		0x110c	/* bus monitor: [1:0] SDA, SCL */
#define HDMI_DDC_SPEED		0x1110
#define   HDMI_DDC_SPEED_STD	0x1f
#define HDMI_DDC_BUF		0x1200	/* 64 bytes */
#define HDMI_DDC_BUF_LEN	64
#define HDMI_DDC_CMD		0x1240	/* [28:20] length, [11:4] offset */
#define   HDMI_DDC_CMD_READ	BIT(3)
#define   HDMI_DDC_CMD_NEXT	BIT(1)	/* buffer read, carry on */
#define   HDMI_DDC_CMD_GO	BIT(0)
#define HDMI_DDC_ADDR_W		0x1254
#define HDMI_DDC_ADDR_R		0x1258
#define HDMI_EDID_ADDR		0x50	/* the sink's EDID, 7-bit */
#define HDMI_DDC_MAX_LEN	256

/* Sideband: the HDMI PHY and the audio PLL. */
#define HDMI_PHY_PORT		0x82
#define   HDMI_PHY_5V		12
#define     HDMI_PHY_5V_OFF	BIT(17)	/* revision 6 only */
#define APLL_PORT		0x8b
/* apll_cr[1] and [2] of stock's AUDIO_HAL_PLL_MODE17 (24.576 MHz, internal) */
#define APLL_MODE17_CR1		0x00090020
#define APLL_MODE17_CR2		0x00063309

#define HDMI_HPD_POLL_MS	500
#define HDMI_EDID_TRIES		3

struct ce5300_hdmi {
	struct drm_bridge	bridge;
	struct pci_dev		*pdev;
	void __iomem		*base;

	struct mutex		lock;		/* sink, notify; link setup vs. hotplug */
	struct ce5300_hdmi_sink	sink;
	void			(*notify)(void *priv);
	void			*notify_priv;

	struct i2c_adapter	ddc;
	struct delayed_work	hpd_work;
	bool			hpd_pending;	/* changed; handle on the next poll */
};

static struct ce5300_hdmi *bridge_to_hdmi(struct drm_bridge *b)
{
	return container_of(b, struct ce5300_hdmi, bridge);
}

static u32 hdmi_read(struct ce5300_hdmi *h, u32 off)
{
	return readl(h->base + off);
}

static void hdmi_write(struct ce5300_hdmi *h, u32 off, u32 val)
{
	writel(val, h->base + off);
}

static void hdmi_update(struct ce5300_hdmi *h, u32 off, u32 mask, u32 val)
{
	hdmi_write(h, off, (hdmi_read(h, off) & ~mask) | (val & mask));
}

static void ce5300_hdmi_5v_on(struct ce5300_hdmi *h)
{
	u32 v;

	hdmi_update(h, HDMI_CLOCKS, HDMI_5V_ENABLE, HDMI_5V_ENABLE);
	if (h->pdev->revision != 6 ||
	    iosf_mbi_read(HDMI_PHY_PORT, MBI_CR_READ, HDMI_PHY_5V, &v))
		return;
	if (v & HDMI_PHY_5V_OFF) {
		iosf_mbi_write(HDMI_PHY_PORT, MBI_CR_WRITE, HDMI_PHY_5V, v & ~HDMI_PHY_5V_OFF);
		dev_info(&h->pdev->dev, "HDMI +5V on (PHY 12 %08x)\n", v);
	}
}

/* Is MCLK the 24.576 MHz the audio divider assumes? Read-only; CEFDK set it. */
static bool ce5300_hdmi_mclk_ok(struct ce5300_hdmi *h)
{
	u32 cr1, cr2;

	if (iosf_mbi_read(APLL_PORT, MBI_CR_READ, 1, &cr1) ||
	    iosf_mbi_read(APLL_PORT, MBI_CR_READ, 2, &cr2))
		return false;
	if (cr1 == APLL_MODE17_CR1 && cr2 == APLL_MODE17_CR2)
		return true;
	dev_err(&h->pdev->dev,
		"audio PLL not at 24.576 MHz (cr1 %#x cr2 %#x); no HDMI audio\n", cr1, cr2);
	return false;
}

/* ---- DDC ----------------------------------------------------------------- */

static void ce5300_hdmi_ddc_on(struct ce5300_hdmi *h)
{
	hdmi_write(h, HDMI_DDC_CTRL, HDMI_DDC_RESET);
	usleep_range(500, 1000);
	hdmi_write(h, HDMI_DDC_CTRL, HDMI_DDC_ON);
	hdmi_write(h, HDMI_DDC_SPEED, HDMI_DDC_SPEED_STD);
}

static int ce5300_hdmi_ddc_wait(struct ce5300_hdmi *h, u32 *st)
{
	return readl_poll_timeout(h->base + HDMI_INT_STATUS, *st, *st & HDMI_INT_DDC,
				  500, 100 * USEC_PER_MSEC);
}

/*
 * Write @off to @addr, then read @len bytes back: the one transaction the
 * engine runs. It fills a 64-byte buffer, says so, and goes on when told.
 */
static int ce5300_hdmi_ddc_read(struct ce5300_hdmi *h, u8 addr, u8 off, u8 *buf,
				unsigned int len)
{
	unsigned int got = 0, n, i;
	u32 st;
	__le32 w;
	int ret = 0;

	if (!len || len > HDMI_DDC_MAX_LEN)
		return -EINVAL;
	if ((hdmi_read(h, HDMI_DDC_LINES) & 3) != 3 ||
	    hdmi_read(h, HDMI_DDC_STATUS) & HDMI_DDC_BUSY) {
		ce5300_hdmi_ddc_on(h);
		if ((hdmi_read(h, HDMI_DDC_LINES) & 3) != 3)
			return -EBUSY;
	}
	hdmi_write(h, HDMI_INT_STATUS, HDMI_INT_DDC);
	hdmi_write(h, HDMI_DDC_ADDR_W, addr << 1);
	hdmi_write(h, HDMI_DDC_ADDR_R, addr << 1 | 1);
	hdmi_write(h, HDMI_DDC_CMD, (len & 0x1ff) << 20 | off << 4 |
		   HDMI_DDC_CMD_READ | HDMI_DDC_CMD_GO);

	while (got < len) {
		ret = ce5300_hdmi_ddc_wait(h, &st);
		if (ret)
			break;
		if (st & HDMI_INT_DDC_ERROR) {
			ret = -ENXIO;
			break;
		}
		n = min_t(unsigned int, len - got, HDMI_DDC_BUF_LEN);
		for (i = 0; i < n; i += 4) {
			put_unaligned_le32(hdmi_read(h, HDMI_DDC_BUF + i), &w);
			memcpy(buf + got + i, &w, min_t(unsigned int, 4, n - i));
		}
		got += n;
		hdmi_write(h, HDMI_INT_STATUS, st & HDMI_INT_DDC);
		if (got < len)
			hdmi_update(h, HDMI_DDC_CMD, HDMI_DDC_CMD_NEXT, HDMI_DDC_CMD_NEXT);
	}
	hdmi_update(h, HDMI_DDC_CMD, HDMI_DDC_CMD_GO, 0);
	hdmi_write(h, HDMI_INT_STATUS, HDMI_INT_DDC);
	return ret;
}

/* The E-DDC read pattern only: offset write + read, segment 0. */
static int ce5300_hdmi_ddc_xfer(struct i2c_adapter *adap, struct i2c_msg *msgs, int num)
{
	struct ce5300_hdmi *h = i2c_get_adapdata(adap);
	int ret;

	if (num && msgs[0].addr == 0x30 && !(msgs[0].flags & I2C_M_RD) &&
	    msgs[0].len == 1 && !msgs[0].buf[0]) {
		msgs++;
		num--;
	}
	if (num != 2 || msgs[0].flags & I2C_M_RD || msgs[0].len != 1 ||
	    !(msgs[1].flags & I2C_M_RD) || msgs[0].addr != msgs[1].addr)
		return -EOPNOTSUPP;
	ret = ce5300_hdmi_ddc_read(h, msgs[0].addr, msgs[0].buf[0], msgs[1].buf,
				   msgs[1].len);
	return ret ? ret : num;
}

static u32 ce5300_hdmi_ddc_func(struct i2c_adapter *adap)
{
	return I2C_FUNC_I2C;
}

static const struct i2c_algorithm ce5300_hdmi_ddc_algo = {
	.master_xfer	= ce5300_hdmi_ddc_xfer,
	.functionality	= ce5300_hdmi_ddc_func,
};

/* Through the adapter, so it is serialized with DRM's own EDID reads. */
static int ce5300_hdmi_edid_read(struct ce5300_hdmi *h, u8 off, u8 *buf, u16 len)
{
	struct i2c_msg msgs[] = {
		{ .addr = HDMI_EDID_ADDR, .len = 1, .buf = &off },
		{ .addr = HDMI_EDID_ADDR, .flags = I2C_M_RD, .len = len, .buf = buf },
	};
	int ret = i2c_transfer(&h->ddc, msgs, ARRAY_SIZE(msgs));

	if (ret == ARRAY_SIZE(msgs))
		return 0;
	return ret < 0 ? ret : -EIO;
}

/* ---- The sink ------------------------------------------------------------ */

static bool edid_block_ok(const u8 *b)
{
	u8 sum = 0;
	unsigned int i;

	for (i = 0; i < 128; i++)
		sum += b[i];
	return !sum;
}

/* Base block and the first extension: where a CEA block says HDMI. */
static int ce5300_hdmi_read_sink(struct ce5300_hdmi *h, struct ce5300_hdmi_sink *s)
{
	static const u8 header[8] = { 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0 };
	u8 *edid, *cea, *d;
	unsigned int i, end, len, tries;
	int ret = -EIO;

	edid = kmalloc(256, GFP_KERNEL);
	if (!edid)
		return -ENOMEM;
	for (tries = 0; tries < HDMI_EDID_TRIES; tries++) {
		if (tries)
			msleep(100);
		ret = ce5300_hdmi_edid_read(h, 0, edid, 128);
		if (ret)
			continue;
		ret = -EBADMSG;
		if (memcmp(edid, header, 8) || !edid_block_ok(edid))
			continue;
		if (!edid[126])
			break;
		ret = ce5300_hdmi_edid_read(h, 128, edid + 128, 128);
		if (!ret && !edid_block_ok(edid + 128))
			ret = -EBADMSG;
		if (!ret)
			break;
	}
	if (ret)
		goto out;
	s->edid = true;

	/* Monitor name descriptor: 13 bytes, LF-terminated, space-padded. */
	for (i = 54; i <= 108; i += 18)
		if (!edid[i] && !edid[i + 1] && edid[i + 3] == 0xfc) {
			memcpy(s->name, &edid[i + 5], 13);
			s->name[strcspn(s->name, "\n")] = 0;
		}

	cea = edid + 128;
	if (!edid[126] || cea[0] != 0x02 || cea[1] < 3)
		goto out;
	s->audio = cea[3] & BIT(6);
	end = min_t(unsigned int, cea[2], 127);
	for (i = 4; i < end; i += len + 1) {
		d = &cea[i];
		len = d[0] & 0x1f;
		if (i + len >= end)
			break;
		switch (d[0] >> 5) {
		case 1:		/* audio data block */
			s->audio |= len >= 3;
			break;
		case 3:		/* vendor-specific */
			if (len >= 5 && d[1] == 0x03 && d[2] == 0x0c && d[3] == 0x00)
				s->hdmi = true;
			break;
		}
	}
out:
	kfree(edid);
	return ret;
}

/* A display came or went (or blipped): read it, then tell DRM and audio. */
static void ce5300_hdmi_sink_changed(struct ce5300_hdmi *h, bool connected)
{
	struct ce5300_hdmi_sink s = { .connected = connected };
	int ret;

	if (connected) {
		ret = ce5300_hdmi_read_sink(h, &s);
		if (ret)
			dev_info(&h->pdev->dev, "display connected, no EDID (%d)\n", ret);
		else
			dev_info(&h->pdev->dev, "display connected: \"%s\", %s%s\n", s.name,
				 s.hdmi ? "HDMI" : "DVI", s.hdmi && s.audio ? " with audio" : "");
	} else {
		dev_info(&h->pdev->dev, "display disconnected\n");
	}

	mutex_lock(&h->lock);
	h->sink = s;
	if (h->notify)
		h->notify(h->notify_priv);
	mutex_unlock(&h->lock);

	drm_bridge_hpd_notify(&h->bridge, connected ? connector_status_connected :
						      connector_status_disconnected);
}

static bool ce5300_hdmi_plugged(struct ce5300_hdmi *h)
{
	u8 buf;

	return hdmi_read(h, HDMI_STATUS) & HDMI_STATUS_HPD ||
	       !ce5300_hdmi_edid_read(h, 0, &buf, 1);
}

static void ce5300_hdmi_hpd_work(struct work_struct *work)
{
	struct ce5300_hdmi *h = container_of(work, struct ce5300_hdmi, hpd_work.work);
	bool hpd = ce5300_hdmi_plugged(h);
	u32 st = hdmi_read(h, HDMI_INT_STATUS) & HDMI_INT_HPD;

	if (st)
		hdmi_write(h, HDMI_INT_STATUS, st);
	/* Debounce: act one poll after the change, on the level then. */
	if (h->hpd_pending)
		ce5300_hdmi_sink_changed(h, hpd);
	h->hpd_pending = st || hpd != READ_ONCE(h->sink.connected);
	schedule_delayed_work(&h->hpd_work, msecs_to_jiffies(HDMI_HPD_POLL_MS));
}

static void ce5300_hdmi_hpd_stop(void *data)
{
	struct ce5300_hdmi *h = data;

	cancel_delayed_work_sync(&h->hpd_work);
}

/* ---- Bridge -------------------------------------------------------------- */

static enum drm_connector_status ce5300_hdmi_detect(struct drm_bridge *bridge,
						    struct drm_connector *connector)
{
	return READ_ONCE(bridge_to_hdmi(bridge)->sink.connected) ?
		connector_status_connected : connector_status_disconnected;
}

static const struct drm_edid *ce5300_hdmi_edid(struct drm_bridge *bridge,
					       struct drm_connector *connector)
{
	return drm_edid_read_ddc(connector, &bridge_to_hdmi(bridge)->ddc);
}

static const struct drm_bridge_funcs ce5300_hdmi_bridge_funcs = {
	.atomic_create_state	= drm_atomic_helper_bridge_create_state,
	.atomic_duplicate_state	= drm_atomic_helper_bridge_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_bridge_destroy_state,
	.detect			= ce5300_hdmi_detect,
	.edid_read		= ce5300_hdmi_edid,
};

/* ---- Audio child --------------------------------------------------------- */

static void ce5300_hdmi_set_notify(struct device *core, void (*fn)(void *priv), void *priv)
{
	struct ce5300_hdmi *h = dev_get_drvdata(core);

	mutex_lock(&h->lock);
	h->notify = fn;
	h->notify_priv = priv;
	mutex_unlock(&h->lock);
}

static void ce5300_hdmi_audio_remove(void *data)
{
	platform_device_unregister(data);
}

static int ce5300_hdmi_add_audio(struct ce5300_hdmi *h)
{
	struct ce5300_hdmi_audio_pdata pdata = {
		.base		= h->base,
		.dma_dev	= &h->pdev->dev,
		.lock		= &h->lock,
		.sink		= &h->sink,
		.core		= &h->pdev->dev,
		.set_notify	= ce5300_hdmi_set_notify,
	};
	struct platform_device *audio;

	if (!ce5300_hdmi_mclk_ok(h))
		return 0;
	audio = platform_device_register_data(&h->pdev->dev, CE5300_HDMI_AUDIO_NAME,
					      PLATFORM_DEVID_AUTO, &pdata, sizeof(pdata));
	if (IS_ERR(audio))
		return PTR_ERR(audio);
	return devm_add_action_or_reset(&h->pdev->dev, ce5300_hdmi_audio_remove, audio);
}

/* ---- Bring-up: named registers only, debugfs ce5300-hdmi/regs ------------ */

static int ce5300_hdmi_regs_show(struct seq_file *m, void *unused)
{
	struct ce5300_hdmi *h = m->private;
	u32 phy = 0;

	seq_printf(m, "clocks %08x\nint_mask %08x\nstatus %08x\nint_status %08x\nopmode %08x\n",
		   hdmi_read(h, HDMI_CLOCKS), hdmi_read(h, HDMI_INT_MASK),
		   hdmi_read(h, HDMI_STATUS), hdmi_read(h, HDMI_INT_STATUS),
		   hdmi_read(h, HDMI_OPMODE));
	if (!iosf_mbi_read(HDMI_PHY_PORT, MBI_CR_READ, HDMI_PHY_5V, &phy))
		seq_printf(m, "phy12 %08x\n", phy);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ce5300_hdmi_regs);

static void ce5300_hdmi_debugfs_remove(void *data)
{
	debugfs_remove_recursive(data);
}

/* ---- PCI ------------------------------------------------------------------ */

static int ce5300_hdmi_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct ce5300_hdmi *h;
	struct dentry *d;
	int ret;

	if (!iosf_mbi_available())
		return -EPROBE_DEFER;

	h = devm_drm_bridge_alloc(&pdev->dev, struct ce5300_hdmi, bridge,
				  &ce5300_hdmi_bridge_funcs);
	if (IS_ERR(h))
		return PTR_ERR(h);
	h->pdev = pdev;
	ret = devm_mutex_init(&pdev->dev, &h->lock);
	if (ret)
		return ret;

	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	h->base = pcim_iomap(pdev, 0, 0);
	if (!h->base)
		return -ENOMEM;
	pci_set_master(pdev);	/* the audio DMA engine */
	pci_set_drvdata(pdev, h);

	ce5300_hdmi_5v_on(h);
	/* As stock does before trusting HPD. Its IRQ is never requested here. */
	hdmi_write(h, HDMI_INT_MASK, HDMI_INT_MASK_STOCK);

	ce5300_hdmi_ddc_on(h);
	h->ddc.owner = THIS_MODULE;
	h->ddc.algo = &ce5300_hdmi_ddc_algo;
	h->ddc.dev.parent = &pdev->dev;
	strscpy(h->ddc.name, "ce5300-hdmi DDC");
	i2c_set_adapdata(&h->ddc, h);
	ret = devm_i2c_add_adapter(&pdev->dev, &h->ddc);
	if (ret)
		return ret;

	d = debugfs_create_dir(DRV_NAME, NULL);
	debugfs_create_file("regs", 0400, d, h, &ce5300_hdmi_regs_fops);
	ret = devm_add_action_or_reset(&pdev->dev, ce5300_hdmi_debugfs_remove, d);
	if (ret)
		return ret;

	ret = ce5300_hdmi_add_audio(h);
	if (ret)
		return ret;

	/* Added before the first notify: drm_bridge_add sets up its HPD lock. */
	h->bridge.of_node = pdev->dev.of_node;
	h->bridge.ops = DRM_BRIDGE_OP_DETECT | DRM_BRIDGE_OP_EDID | DRM_BRIDGE_OP_HPD;
	h->bridge.type = DRM_MODE_CONNECTOR_HDMIA;
	h->bridge.ddc = &h->ddc;
	ret = devm_drm_bridge_add(&pdev->dev, &h->bridge);
	if (ret)
		return ret;

	hdmi_write(h, HDMI_INT_STATUS, HDMI_INT_HPD);
	ce5300_hdmi_sink_changed(h, ce5300_hdmi_plugged(h));
	INIT_DELAYED_WORK(&h->hpd_work, ce5300_hdmi_hpd_work);
	ret = devm_add_action_or_reset(&pdev->dev, ce5300_hdmi_hpd_stop, h);
	if (ret)
		return ret;
	schedule_delayed_work(&h->hpd_work, msecs_to_jiffies(HDMI_HPD_POLL_MS));
	return 0;
}

static const struct pci_device_id ce5300_hdmi_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x2e63) },
	{ }
};
MODULE_DEVICE_TABLE(pci, ce5300_hdmi_ids);

static struct pci_driver ce5300_hdmi_driver = {
	.name		= DRV_NAME,
	.id_table	= ce5300_hdmi_ids,
	.probe		= ce5300_hdmi_probe,
};
module_pci_driver(ce5300_hdmi_driver);

MODULE_DESCRIPTION("Intel CE5300 HDMI transmitter");
MODULE_LICENSE("GPL");
