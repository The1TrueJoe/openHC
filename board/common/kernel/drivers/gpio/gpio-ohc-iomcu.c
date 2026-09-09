// SPDX-License-Identifier: GPL-2.0
/*
 * openHC — Control4 IO microcontroller as a GPIO chip.
 *
 * The relays and contacts on a Control4 controller do not hang off the SoC.
 * They hang off a companion microcontroller (TI Stellaris LM3S1162 on the HC
 * family, TI Tiva TM4C1231D5 on the EA family) reached over a host UART with a
 * DLE/STX framed protocol.
 *
 * This driver puts that behind a standard gpio_chip, so a relay is a GPIO line
 * like any other: gpioset closes it, libgpiod sees it, and every tool that
 * knows Linux works without knowing anything about Control4. Userspace daemons
 * become clients of the kernel rather than owners of a UART.
 *
 * A LINE DISCIPLINE, not serdev: serdev binds through device tree or ACPI, and
 * these are x86 PCs whose BIOS has no node describing a Control4 co-processor,
 * so nothing is ever enumerated to bind to. TIOCSETD needs no enumeration — the
 * way PPP, SLIP and n_gsm attach — and it leaves the choice of WHICH port in
 * userspace, where the board description already lives.
 *
 * Copyright (C) 2026 openHC
 */

#define pr_fmt(fmt) "ohc-iomcu: " fmt

#include <linux/completion.h>
#include <linux/gpio/driver.h>
#include <media/rc-core.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/printk.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/tty.h>
#include <linux/tty_ldisc.h>
#include <linux/workqueue.h>

/*
 * N_DEVELOPMENT is the slot the kernel reserves for out-of-tree line
 * disciplines. Overridable because the numbering is a uapi constant that has
 * grown over time, and a collision should be a loud module-load failure the
 * operator can work around, not a reason to patch a header.
 */
#ifdef N_DEVELOPMENT
#define OHC_LDISC_DEFAULT N_DEVELOPMENT
#else
#define OHC_LDISC_DEFAULT 29
#endif

static int ldisc_num = OHC_LDISC_DEFAULT;
module_param(ldisc_num, int, 0444);
MODULE_PARM_DESC(ldisc_num, "line discipline number to register (default N_DEVELOPMENT)");

/*
 * Writable, because this is built in and a built-in's parameters can otherwise
 * only come from the kernel command line. The init script writes these from
 * board.env then attaches the discipline; geometry is latched at attach, so a
 * later write affects the next attach and never a registered chip.
 */
static int relays = 4;
module_param(relays, int, 0644);
MODULE_PARM_DESC(relays, "number of relay outputs the board fits");

static int contacts = 4;
module_param(contacts, int, 0644);
MODULE_PARM_DESC(contacts, "number of contact inputs the board fits");

/*
 * IR emitters as the case is labelled: rear jacks, then an internal front
 * blaster. Zero of both means no IR and no rc device.
 */
static int ir_out;
module_param(ir_out, int, 0644);
MODULE_PARM_DESC(ir_out, "number of rear IR output jacks");

static int ir_blaster;
module_param(ir_blaster, int, 0644);
MODULE_PARM_DESC(ir_blaster, "1 if the board has an internal front blaster");

static int ir_in;
module_param(ir_in, int, 0644);
MODULE_PARM_DESC(ir_in, "1 if the board has an IR receiver");

static int poll_ms = 200;
module_param(poll_ms, int, 0644);
MODULE_PARM_DESC(poll_ms, "contact poll interval in milliseconds (0 disables)");

/* Framing. */
#define DLE 0x10
#define STX 0x02

/* Opcodes, established against live hardware. */
#define OP_PRODUCT_NAME     0x24
#define OP_FIRMWARE_VERSION 0x34
#define OP_RELAY_GET        0x54
#define OP_RELAY_TOGGLE     0x56
#define OP_CONTACT_GET      0x74
#define OP_IROUT_SEND       0x66
#define OP_IRIN_SET_CAPTURE 0x77
#define OP_IRIN_CAPTURED    0x97

#define OHC_MAX_PAYLOAD 512
#define OHC_TIMEOUT_MS  600

/*
 * The IR side, decoded from the vendor firmware.
 *
 * IROUT_SEND takes 14 fixed bytes then one u16 per burst duration. The output
 * selector is a 24-bit BITMASK — on an HC-800, bits 0..5 are the rear jacks and
 * bit 6 is the internal front blaster.
 *
 * The mask travels in the same frame as the durations, so choosing an output is
 * per-transmission and not a mode the part sits in. Hence one lirc device per
 * output with its bit fixed, rather than one device plus s_tx_mask: with a mask
 * the second process to set it silently redirects the first.
 *
 * The carrier field is a Pronto word, and the firmware DIVIDES BY IT. Zero
 * there is a divide-by-zero: UsageFault, HardFault, and a part that answers
 * nothing until it is power cycled. Never send 0.
 */
#define IR_PRONTO_HZ    4145146u  /* 1e6 / 0.241246 */
#define IR_SYS_HZ       50000000u /* LM3S system clock, for capture periods */
#define IR_FIXED_BYTES  14
/* Derived: a longer durations array overruns one frame. */
#define IR_MAX_DURATIONS ((OHC_MAX_PAYLOAD - IR_FIXED_BYTES) / 2)

enum rx_state {
	RX_IDLE,	/* hunting for DLE */
	RX_DLE,		/* saw DLE, expecting STX */
	RX_BODY,	/* collecting body bytes */
	RX_BODY_DLE,	/* saw DLE inside the body: stuffed, or a new frame */
};

struct ohc_iomcu;

/* One IR output: a lirc device that always drives its own bit. */
struct ohc_emitter {
	struct rc_dev *rc;
	struct ohc_iomcu *mcu;
	u32 mask;		/* the bit this node owns */
	u32 carrier;		/* Hz, per device */
	char *name;
};

struct ohc_frame {
	u8 opcode;
	u8 seq;
	u8 payload[OHC_MAX_PAYLOAD];
	int len;
};

struct ohc_iomcu {
	struct tty_struct *tty;
	struct gpio_chip gc;
	struct device *dev;

	/* One conversation at a time: the MCU answers one question at a time. */
	struct mutex io_lock;

	/* The reply we are waiting for, and how it gets back to the waiter. */
	struct completion reply;
	spinlock_t rx_lock;
	u8 want_seq;
	bool want_reply;
	struct ohc_frame reply_frame;

	/* Decoder. */
	enum rx_state state;
	u8 body[OHC_MAX_PAYLOAD + 8];
	int body_len;

	u8 seq;

	/*
	 * Contact state, refreshed by the poller. Relays are deliberately NOT
	 * cached: they are read rarely and a stale relay reading is a worse
	 * thing to hand out than a slow one.
	 */
	u32 contact_mask;
	bool contacts_valid;

	struct delayed_work poll_work;
	/*
	 * Identify and register the chip AFTER open() returns. See ohc_probe.
	 */
	struct delayed_work probe_work;
	bool chip_added;
	const char **names;

	/*
	 * Latched at attach. Reading the module parameters on every access
	 * would let a sysfs write shift the line map underneath a gpiochip that
	 * has already told userspace what its lines are.
	 */
	int n_relays;
	int n_contacts;

	/* One lirc device per emitter; the receiver is separate, because it is
	 * the only one that should present an input device. */
	struct ohc_emitter *emitters;
	int n_emitters;
	struct rc_dev *rx;

	/*
	 * Diagnostics. "No reply" has three very different causes — nothing was
	 * transmitted, nothing came back, or something came back and the decoder
	 * rejected it — and they are indistinguishable from the timeout alone.
	 */
	unsigned long tx_bytes;
	unsigned long rx_bytes;
	unsigned long rx_frames;
	unsigned long rx_bad_csum;
	u8 first_rx[16];
	int first_rx_len;
};

/* Negated 8-bit sum, plus one. */
static u8 ohc_checksum(const u8 *body, int len)
{
	u8 sum = 0;
	int i;

	for (i = 0; i < len; i++)
		sum += body[i];
	return (u8)(~sum) + 1;
}

static int ohc_write_frame(struct ohc_iomcu *mcu, u8 opcode, u8 seq,
			   const u8 *payload, int len)
{
	u8 body[8 + OHC_MAX_PAYLOAD];
	u8 *out;
	int body_len = 0, n = 0, i, ret;
	u8 ck;

	if (len > OHC_MAX_PAYLOAD)
		return -EINVAL;

	body[body_len++] = opcode;
	body[body_len++] = seq;
	body[body_len++] = 0;			/* flags */
	body[body_len++] = (u8)(len >> 8);
	body[body_len++] = (u8)len;
	if (len)
		memcpy(body + body_len, payload, len);
	body_len += len;
	ck = ohc_checksum(body, body_len);

	/* Worst case every byte is DLE and gets stuffed. */
	out = kmalloc(2 + (body_len + 1) * 2, GFP_KERNEL);
	if (!out)
		return -ENOMEM;

	out[n++] = DLE;
	out[n++] = STX;
	for (i = 0; i < body_len; i++) {
		out[n++] = body[i];
		if (body[i] == DLE)
			out[n++] = DLE;
	}
	out[n++] = ck;
	if (ck == DLE)
		out[n++] = DLE;

	ret = mcu->tty->ops->write(mcu->tty, out, n);
	if (ret > 0)
		mcu->tx_bytes += ret;
	if (ret >= 0 && ret < n)
		pr_warn("short write: %d of %d bytes\n", ret, n);
	kfree(out);
	return ret < 0 ? ret : 0;
}

/*
 * Fire and forget, for the opcodes the firmware does not answer.
 *
 * IRIN_SET_CAPTURE is one: there is no reply to correlate, so a wrong argument
 * is silent and the only symptom is a receiver that never reports anything.
 */
static int ohc_send(struct ohc_iomcu *mcu, u8 opcode, const u8 *payload, int len)
{
	int ret;

	mutex_lock(&mcu->io_lock);
	ret = ohc_write_frame(mcu, opcode, ++mcu->seq, payload, len);
	mutex_unlock(&mcu->io_lock);
	return ret;
}

/* Ask, and wait for the answer with the matching sequence number. */
static int ohc_request(struct ohc_iomcu *mcu, u8 opcode, const u8 *payload,
		       int len, struct ohc_frame *out)
{
	unsigned long flags;
	long left;
	int ret;
	u8 seq;

	mutex_lock(&mcu->io_lock);
	seq = ++mcu->seq;

	spin_lock_irqsave(&mcu->rx_lock, flags);
	mcu->want_seq = seq;
	mcu->want_reply = true;
	spin_unlock_irqrestore(&mcu->rx_lock, flags);
	reinit_completion(&mcu->reply);

	ret = ohc_write_frame(mcu, opcode, seq, payload, len);
	if (ret)
		goto out;

	left = wait_for_completion_interruptible_timeout(
		&mcu->reply, msecs_to_jiffies(OHC_TIMEOUT_MS));
	if (left <= 0) {
		ret = left == 0 ? -ETIMEDOUT : -ERESTARTSYS;
		goto out;
	}
	if (out)
		*out = mcu->reply_frame;
out:
	spin_lock_irqsave(&mcu->rx_lock, flags);
	mcu->want_reply = false;
	spin_unlock_irqrestore(&mcu->rx_lock, flags);
	mutex_unlock(&mcu->io_lock);
	return ret;
}

/*
 * RELAY_GET/RELAY_TOGGLE take a 0-BASED INDEX and answer [index, state].
 *
 * Not a bitmask: the firmware echoes any selector verbatim, so a GET cannot
 * tell the two apart, but a TOGGLE sweep on a live HC-800 shows 0x00..0x03
 * each flipping one relay and 0x04 up doing nothing. Read as a bitmask, relays
 * 2 and 3 became 0x04 and 0x08 and addressed nothing — which is why the board
 * looked like it had two dead relays.
 */
static int ohc_relay_get(struct ohc_iomcu *mcu, int index, bool *on)
{
	struct ohc_frame f;
	u8 sel = (u8)index;
	int ret;

	ret = ohc_request(mcu, OP_RELAY_GET, &sel, 1, &f);
	if (ret)
		return ret;
	if (f.len < 2)
		return -EPROTO;
	if (f.payload[0] != sel)
		return -EPROTO;
	*on = f.payload[1] != 0;
	return 0;
}

static int ohc_relay_toggle(struct ohc_iomcu *mcu, int index, bool *now)
{
	struct ohc_frame f;
	u8 sel = (u8)index;
	int ret;

	ret = ohc_request(mcu, OP_RELAY_TOGGLE, &sel, 1, &f);
	if (ret)
		return ret;
	if (f.len < 2 || f.payload[0] != sel)
		return -EPROTO;
	*now = f.payload[1] != 0;
	return 0;
}

/*
 * u32 LITTLE-endian mask, bit N = contact N; 1 = CLOSED.
 *
 * Byte 0 carries contacts 1-8. Read as big-endian for a long time with nothing
 * catching it: an all-open board answers 00 00 00 00, identical either way.
 * Closing contact 4 answers 08 00 00 00 — bit 3 little-endian, which is the
 * fourth contact; big-endian that would be bit 27.
 */
static int ohc_contacts_get(struct ohc_iomcu *mcu, u32 *mask)
{
	struct ohc_frame f;
	int ret;

	ret = ohc_request(mcu, OP_CONTACT_GET, NULL, 0, &f);
	if (ret)
		return ret;
	if (f.len < 4)
		return -EPROTO;
	*mask = ((u32)f.payload[3] << 24) | ((u32)f.payload[2] << 16) |
		((u32)f.payload[1] << 8) | f.payload[0];
	return 0;
}

/* ---- infrared ------------------------------------------------------------ */

/*
 * rc-core hands us durations in MICROSECONDS; the firmware wants them in
 * carrier periods, which is also what Pronto uses. Convert with the carrier we
 * were told, and refuse rather than guess if we were told nothing.
 */
static int ohc_tx_ir(struct rc_dev *rcdev, unsigned int *txbuf, unsigned int count)
{
	struct ohc_emitter *em = rcdev->priv;
	struct ohc_iomcu *mcu = em->mcu;
	unsigned int i, n;
	u32 carrier, word;
	u8 *payload;
	int ret;

	carrier = em->carrier ? em->carrier : 38000;
	word = IR_PRONTO_HZ / carrier;
	if (!word)
		return -EINVAL;		/* would be a divide-by-zero in the firmware */

	/* Truncating would transmit an unrecognisable code and report success. */
	if (count > IR_MAX_DURATIONS)
		return -EMSGSIZE;
	n = count;
	payload = kzalloc(IR_FIXED_BYTES + n * 2, GFP_KERNEL);
	if (!payload)
		return -ENOMEM;

	payload[0] = 1;			/* mode: < 2 skips the repeat block */
	/* This node's own bit. No shared selection to race over. */
	payload[1] = (em->mask >> 16) & 0xff;
	payload[2] = (em->mask >> 8) & 0xff;
	payload[3] = em->mask & 0xff;
	/* payload[4..7] are read and unused on this path */
	payload[8] = word >> 8;		/* carrier, big-endian */
	payload[9] = word & 0xff;
	/* payload[10..13]: repeat count and offset, both zero */
	for (i = 0; i < n; i++) {
		u32 periods = ((u64)txbuf[i] * carrier) / 1000000u;

		if (periods > 0x7fff)
			periods = 0x7fff;
		payload[IR_FIXED_BYTES + i * 2] = periods >> 8;
		payload[IR_FIXED_BYTES + i * 2 + 1] = periods & 0xff;
	}

	ret = ohc_request(mcu, OP_IROUT_SEND, payload, IR_FIXED_BYTES + n * 2, NULL);
	kfree(payload);
	return ret ? ret : (int)n;
}

static int ohc_tx_carrier(struct rc_dev *rcdev, u32 carrier)
{
	struct ohc_emitter *em = rcdev->priv;

	/* The firmware divides by the derived word; a carrier this low would
	 * round it to zero and take the part down with it. */
	if (carrier < 1000 || IR_PRONTO_HZ / carrier == 0)
		return -EINVAL;
	em->carrier = carrier;
	return 0;
}

/* A capture from the front receiver: [timer period][durations, bit 15 = mark] */
static void ohc_ir_capture(struct ohc_iomcu *mcu, const u8 *p, int len)
{
	struct ir_raw_event ev = {};
	u32 period, carrier;
	int i;

	if (!mcu->rx || len < 4)
		return;
	period = ((u32)p[0] << 8) | p[1];
	if (!period)
		return;
	carrier = IR_SYS_HZ / period;
	if (!carrier)
		return;

	/*
	 * Lead with the measured carrier: it reaches lirc as
	 * LIRC_MODE2_FREQUENCY, and is the only place the frequency survives —
	 * the durations that follow are microseconds.
	 */
	ev.carrier_report = 1;
	ev.carrier = carrier;
	ir_raw_event_store(mcu->rx, &ev);

	for (i = 2; i + 1 < len; i += 2) {
		u16 w = ((u16)p[i] << 8) | p[i + 1];

		memset(&ev, 0, sizeof(ev));
		if (!(w & 0x7fff))
			break;			/* padding, not a zero-length burst */
		ev.pulse = !!(w & 0x8000);
		/* periods -> microseconds, the unit rc-core wants */
		ev.duration = ((u64)(w & 0x7fff) * 1000000u) / carrier;
		ir_raw_event_store(mcu->rx, &ev);
	}
	/* The capture is complete; say so rather than leaving a reader to infer
	 * it from a trailing gap this protocol never sends. */
	memset(&ev, 0, sizeof(ev));
	ev.timeout = 1;
	ir_raw_event_store(mcu->rx, &ev);
	ir_raw_event_handle(mcu->rx);
}

/* ---- gpio_chip ---------------------------------------------------------- */
/*
 * Line map: relays first as outputs, then contacts as inputs. Relays lead
 * because they are the lines somebody will address by number in a hurry, and
 * an off-by-one that closes a relay is worse than one that misreads a contact.
 */

static bool ohc_is_relay(struct ohc_iomcu *mcu, unsigned int off)
{
	return off < (unsigned int)mcu->n_relays;
}

static int ohc_gpio_get_direction(struct gpio_chip *gc, unsigned int off)
{
	struct ohc_iomcu *mcu = gpiochip_get_data(gc);

	return ohc_is_relay(mcu, off) ? GPIO_LINE_DIRECTION_OUT
				      : GPIO_LINE_DIRECTION_IN;
}

/*
 * Accepted for relays too, deliberately: GPIOHANDLE_REQUEST_INPUT is how every
 * libgpiod-1.x tool asks to READ a line, so refusing it makes
 * `gpioget $(gpiofind relay1)` fail with EINVAL on a line the driver can read
 * perfectly well. Honoured as read-only access; get_direction still says OUT.
 */
static int ohc_gpio_direction_input(struct gpio_chip *gc, unsigned int off)
{
	return 0;
}


static int ohc_gpio_set(struct gpio_chip *gc, unsigned int off, int value);

static int ohc_gpio_direction_output(struct gpio_chip *gc, unsigned int off,
				     int value)
{
	struct ohc_iomcu *mcu = gpiochip_get_data(gc);

	if (!ohc_is_relay(mcu, off))
		return -EINVAL;
	return ohc_gpio_set(gc, off, value);
}

static int ohc_gpio_get(struct gpio_chip *gc, unsigned int off)
{
	struct ohc_iomcu *mcu = gpiochip_get_data(gc);
	bool on;

	if (ohc_is_relay(mcu, off)) {
		if (ohc_relay_get(mcu, off, &on))
			return -EIO;
		return on;
	}

	/*
	 * Contacts come from the poller's cache. Falling back to a live read
	 * covers the window before the first poll completes, and the case where
	 * polling is switched off with poll_ms=0.
	 */
	if (!mcu->contacts_valid) {
		u32 mask;

		if (ohc_contacts_get(mcu, &mask))
			return -EIO;
		mcu->contact_mask = mask;
		mcu->contacts_valid = true;
	}
	return !!(mcu->contact_mask & BIT(off - mcu->n_relays));
}

/*
 * Returns an error rather than swallowing one. A relay that did not move is
 * something the caller needs to know about — it may be switching a real load.
 */
static int ohc_gpio_set(struct gpio_chip *gc, unsigned int off, int value)
{
	struct ohc_iomcu *mcu = gpiochip_get_data(gc);
	bool on, now;
	int ret;

	if (!ohc_is_relay(mcu, off))
		return -EINVAL;

	/*
	 * The firmware has no SET opcode, only TOGGLE and GET, so this reads
	 * first and toggles only on a mismatch. That difference is not
	 * cosmetic: gpiod_set_value() is expected to be idempotent, and a
	 * toggle-only implementation would invert a relay every time something
	 * wrote the value it already had.
	 */
	ret = ohc_relay_get(mcu, off, &on);
	if (ret)
		return ret;
	if (on == !!value)
		return 0;
	ret = ohc_relay_toggle(mcu, off, &now);
	if (ret)
		return ret;

	/*
	 * The firmware acknowledged a state that is not the one asked for.
	 * Saying so beats reporting success for a relay that did not move.
	 */
	return (now == !!value) ? 0 : -EIO;
}

/*
 * Contacts have no unsolicited notification — the host polls. Doing it once
 * here is the point of putting this in the kernel: five clients asking "is the
 * door open" become five cached reads, not five UART round trips.
 */
static void ohc_poll(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, poll_work);
	u32 mask;

	if (!ohc_contacts_get(mcu, &mask)) {
		mcu->contact_mask = mask;
		mcu->contacts_valid = true;
	}

	if (poll_ms > 0)
		schedule_delayed_work(&mcu->poll_work, msecs_to_jiffies(poll_ms));
}

/* ---- decoder ------------------------------------------------------------ */

static void ohc_frame_complete(struct ohc_iomcu *mcu)
{
	int body_len = mcu->body_len;
	unsigned long flags;
	u8 ck, opcode, seq;
	int len;

	if (body_len < 6)		/* header + at least a checksum */
		return;

	ck = mcu->body[body_len - 1];
	if (ohc_checksum(mcu->body, body_len - 1) != ck) {
		mcu->rx_bad_csum++;
		return;
	}
	mcu->rx_frames++;

	opcode = mcu->body[0];
	seq = mcu->body[1];
	len = ((int)mcu->body[3] << 8) | mcu->body[4];
	if (len > body_len - 6)
		len = body_len - 6;

	spin_lock_irqsave(&mcu->rx_lock, flags);
	if (mcu->want_reply && seq == mcu->want_seq) {
		mcu->reply_frame.opcode = opcode;
		mcu->reply_frame.seq = seq;
		mcu->reply_frame.len = len;
		if (len > 0)
			memcpy(mcu->reply_frame.payload, mcu->body + 5, len);
		mcu->want_reply = false;
		spin_unlock_irqrestore(&mcu->rx_lock, flags);
		complete(&mcu->reply);
		return;
	}
	spin_unlock_irqrestore(&mcu->rx_lock, flags);

	/*
	 * Unsolicited: a remote was pressed at the front receiver. This is the
	 * whole reason the decoder keeps frames it did not ask for.
	 */
	if (opcode == OP_IRIN_CAPTURED)
		ohc_ir_capture(mcu, mcu->body + 5, len);
}

static void ohc_feed(struct ohc_iomcu *mcu, const u8 *buf, int count)
{
	int i;

	mcu->rx_bytes += count;
	if (mcu->first_rx_len < (int)sizeof(mcu->first_rx)) {
		int room = (int)sizeof(mcu->first_rx) - mcu->first_rx_len;
		int take = count < room ? count : room;

		memcpy(mcu->first_rx + mcu->first_rx_len, buf, take);
		mcu->first_rx_len += take;
	}

	for (i = 0; i < count; i++) {
		u8 b = buf[i];

		switch (mcu->state) {
		case RX_IDLE:
			if (b == DLE)
				mcu->state = RX_DLE;
			break;
		case RX_DLE:
			/*
			 * Resynchronise rather than wedge. A UART sees line
			 * noise and half-frames on every reset, and a decoder
			 * that can only be recovered by reloading the module
			 * is not usable on real hardware.
			 */
			mcu->state = (b == STX) ? RX_BODY : RX_IDLE;
			mcu->body_len = 0;
			break;
		case RX_BODY:
			if (b == DLE) {
				mcu->state = RX_BODY_DLE;
			} else if (mcu->body_len < (int)sizeof(mcu->body)) {
				mcu->body[mcu->body_len++] = b;
			} else {
				mcu->state = RX_IDLE;
			}
			break;
		case RX_BODY_DLE:
			if (b == DLE) {
				/* Stuffed: one real DLE. */
				if (mcu->body_len < (int)sizeof(mcu->body))
					mcu->body[mcu->body_len++] = DLE;
				mcu->state = RX_BODY;
			} else if (b == STX) {
				/* A new frame started; the old one was junk. */
				ohc_frame_complete(mcu);
				mcu->body_len = 0;
				mcu->state = RX_BODY;
			} else {
				mcu->state = RX_IDLE;
			}
			break;
		}
	}

	/*
	 * The protocol has no end-of-frame marker: a frame is complete when the
	 * next one starts or the line goes quiet. Length says how much payload
	 * to expect, so check for a whole frame on every chunk.
	 */
	while (mcu->state == RX_BODY && mcu->body_len >= 6) {
		int want = 5 + (((int)mcu->body[3] << 8) | mcu->body[4]) + 1;
		int saved;

		/*
		 * A corrupt length would otherwise stall the decoder until the
		 * next DLE/STX happened to resynchronise it. Resync now.
		 */
		if (want < 6 || want > (int)sizeof(mcu->body)) {
			mcu->state = RX_IDLE;
			mcu->body_len = 0;
			break;
		}
		if (mcu->body_len < want)
			break;

		saved = mcu->body_len;
		mcu->body_len = want;
		ohc_frame_complete(mcu);

		/*
		 * Loop rather than return: a reply and an unsolicited IR capture
		 * can land in the same read, and completing only the first would
		 * leave the second stuck until the next byte happened to arrive.
		 */
		mcu->body_len = saved - want;
		if (mcu->body_len > 0)
			memmove(mcu->body, mcu->body + want, mcu->body_len);
	}
}

/* ---- line discipline ---------------------------------------------------- */

/*
 * Register the IR side: one lirc device per emitter, plus one for the receiver.
 * Reasoning at IR_PRONTO_HZ.
 *
 * Each node is NAMED — the number depends on probe order, so DEV_NAME in
 * /sys/class/rc/rcN/uevent is the stable handle. Failure is not fatal: a board
 * with working relays and no IR is worth having.
 */
/* Takes ownership of @name; rc-core keeps the pointer rather than copying. */
static int ohc_register_emitter(struct ohc_iomcu *mcu, struct ohc_emitter *em,
				u32 bit, char *name)
{
	struct rc_dev *rc;
	int ret;

	if (!name)
		return -ENOMEM;
	em->name = name;

	rc = rc_allocate_device(RC_DRIVER_IR_RAW_TX);
	if (!rc) {
		kfree(em->name);
		em->name = NULL;
		return -ENOMEM;
	}

	em->mcu = mcu;
	em->mask = bit;
	em->carrier = 38000;	/* so a bare `ir-ctl -S` does something sane */

	rc->priv = em;
	rc->driver_name = "ohc-iomcu";
	rc->device_name = em->name;
	rc->tx_ir = ohc_tx_ir;
	rc->s_tx_carrier = ohc_tx_carrier;

	ret = rc_register_device(rc);
	if (ret) {
		rc_free_device(rc);
		kfree(em->name);
		em->name = NULL;
		return ret;
	}
	em->rc = rc;
	return 0;
}

static void ohc_register_rc(struct ohc_iomcu *mcu)
{
	int n = ir_out + (ir_blaster ? 1 : 0);
	struct rc_dev *rx;
	int i, ok = 0;

	if (n > 0) {
		mcu->emitters = kcalloc(n, sizeof(*mcu->emitters), GFP_KERNEL);
		if (!mcu->emitters)
			return;
		mcu->n_emitters = n;

		/*
		 * Jacks are numbered from 1 to match the silkscreen; the front
		 * blaster is not a jack and is named, not numbered.
		 */
		for (i = 0; i < ir_out; i++) {
			int ret = ohc_register_emitter(mcu, &mcu->emitters[i], 1u << i,
						       kasprintf(GFP_KERNEL,
								 "openHC IR out %d", i + 1));

			if (ret)
				pr_warn("%s: IR out %d did not register: %d\n",
					mcu->tty->name, i + 1, ret);
			else
				ok++;
		}
		if (ir_blaster) {
			int ret = ohc_register_emitter(mcu, &mcu->emitters[ir_out],
						       1u << ir_out,
						       kstrdup("openHC IR front blaster",
							       GFP_KERNEL));

			if (ret)
				pr_warn("%s: front blaster did not register: %d\n",
					mcu->tty->name, ret);
			else
				ok++;
		}
	}

	if (ir_in) {
		u8 on = 1;

		/*
		 * The only node with an input device: it is the only one that
		 * can produce a key press.
		 */
		rx = rc_allocate_device(RC_DRIVER_IR_RAW);
		if (rx) {
			rx->priv = mcu;
			rx->driver_name = "ohc-iomcu";
			rx->device_name = "openHC IR front receiver";
			rx->input_phys = "ohc-iomcu/front";
			rx->map_name = RC_MAP_EMPTY;
			rx->allowed_protocols = RC_PROTO_BIT_ALL_IR_DECODER;
			if (rc_register_device(rx)) {
				rc_free_device(rx);
			} else {
				mcu->rx = rx;
				/*
				 * Ask the firmware to report what it hears.
				 * Fire and forget — it sends no reply — so a
				 * wrong argument fails silently, and the
				 * symptom is a receiver that never fires.
				 */
				ohc_send(mcu, OP_IRIN_SET_CAPTURE, &on, 1);
			}
		}
	}

	if (ok || mcu->rx)
		pr_info("%s: %d IR emitter(s)%s\n", mcu->tty->name, ok,
			mcu->rx ? " and a receiver" : "");
}

/*
 * Identify the microcontroller, then register the chip.
 *
 * THIS CANNOT HAPPEN IN open(). tty_set_ldisc() holds tty->ldisc_sem across the
 * ldisc's open(), and the receive path needs that same semaphore to deliver the
 * reply — so a request from open() times out every time, and the part looks
 * dead to this driver while answering userspace perfectly.
 */
static void ohc_probe(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, probe_work);
	struct ohc_frame f;
	int i, lines, ret;

	ret = ohc_request(mcu, OP_FIRMWARE_VERSION, NULL, 0, &f);
	if (ret) {
		/*
		 * No chip. A gpiochip standing in for a part that is not
		 * answering is one whose every read is a lie, and it would be
		 * indistinguishable from working hardware until somebody
		 * trusted a contact.
		 */
		pr_warn("no reply from the IO microcontroller on %s; no gpiochip registered\n",
			mcu->tty->name);
		pr_warn("  tx=%lu rx=%lu frames=%lu badcsum=%lu\n",
			mcu->tx_bytes, mcu->rx_bytes, mcu->rx_frames, mcu->rx_bad_csum);
		if (mcu->first_rx_len)
			print_hex_dump(KERN_WARNING, "ohc-iomcu: first rx: ",
				       DUMP_PREFIX_NONE, 16, 1,
				       mcu->first_rx, mcu->first_rx_len, false);
		return;
	}

	lines = mcu->n_relays + mcu->n_contacts;
	if (!lines) {
		/* IR-only board: no gpiochip to register, but the rc device below
		 * is still worth having. */
		ohc_register_rc(mcu);
		return;
	}
	mcu->names = kcalloc(lines, sizeof(char *), GFP_KERNEL);
	if (!mcu->names)
		return;
	/*
	 * Named lines, so `gpiofind relay1` works. ONE-BASED, because that is
	 * what is printed on the back of the box; the line offset and the wire
	 * selector stay zero-based, as they must.
	 */
	for (i = 0; i < lines; i++) {
		mcu->names[i] = kasprintf(GFP_KERNEL, "%s%d",
					  i < mcu->n_relays ? "relay" : "contact",
					  (i < mcu->n_relays ? i : i - mcu->n_relays) + 1);
		if (!mcu->names[i])
			goto err_names;
	}

	mcu->gc.label = "ohc-iomcu";
	mcu->gc.owner = THIS_MODULE;
	mcu->gc.base = -1;
	mcu->gc.ngpio = lines;
	mcu->gc.names = mcu->names;
	mcu->gc.get_direction = ohc_gpio_get_direction;
	mcu->gc.direction_input = ohc_gpio_direction_input;
	mcu->gc.direction_output = ohc_gpio_direction_output;
	mcu->gc.get = ohc_gpio_get;
	mcu->gc.set = ohc_gpio_set;
	/* Every access is a UART round trip. */
	mcu->gc.can_sleep = true;

	if (gpiochip_add_data(&mcu->gc, mcu))
		goto err_names;
	mcu->chip_added = true;

	if (poll_ms > 0)
		schedule_delayed_work(&mcu->poll_work, msecs_to_jiffies(poll_ms));

	pr_info("%s: %d relays, %d contacts as GPIO lines\n",
		mcu->tty->name, mcu->n_relays, mcu->n_contacts);

	ohc_register_rc(mcu);
	return;

err_names:
	for (i = 0; i < lines; i++)
		kfree(mcu->names[i]);
	kfree(mcu->names);
	mcu->names = NULL;
}

static int ohc_ldisc_open(struct tty_struct *tty)
{
	struct ohc_iomcu *mcu;

	if (!tty->ops->write)
		return -EOPNOTSUPP;
	if (relays < 0 || contacts < 0 || relays > 32 || contacts > 32)
		return -EINVAL;
	/*
	 * A board may have IR and no GPIO lines at all, so "no relays and no
	 * contacts" is not a reason to refuse the port — only "nothing at all
	 * behind it" is.
	 */
	if (relays + contacts == 0 && ir_out + ir_blaster + ir_in == 0)
		return -EINVAL;

	mcu = kzalloc(sizeof(*mcu), GFP_KERNEL);
	if (!mcu)
		return -ENOMEM;

	mcu->tty = tty;
	mcu->n_relays = relays;
	mcu->n_contacts = contacts;
	mutex_init(&mcu->io_lock);
	spin_lock_init(&mcu->rx_lock);
	init_completion(&mcu->reply);
	INIT_DELAYED_WORK(&mcu->poll_work, ohc_poll);
	INIT_DELAYED_WORK(&mcu->probe_work, ohc_probe);
	tty->disc_data = mcu;
	/* See ohc_ldisc_receive_buf2: zero here means nothing is ever delivered. */
	tty->receive_room = 65536;

	/* Talk to the part once this returns and the ldisc lock is released. */
	schedule_delayed_work(&mcu->probe_work, msecs_to_jiffies(50));
	return 0;
}

static void ohc_ldisc_close(struct tty_struct *tty)
{
	struct ohc_iomcu *mcu = tty->disc_data;
	int i, lines;

	if (!mcu)
		return;

	cancel_delayed_work_sync(&mcu->probe_work);
	cancel_delayed_work_sync(&mcu->poll_work);
	for (i = 0; i < mcu->n_emitters; i++) {
		if (mcu->emitters[i].rc)
			rc_unregister_device(mcu->emitters[i].rc);
		kfree(mcu->emitters[i].name);
	}
	kfree(mcu->emitters);
	if (mcu->rx)
		rc_unregister_device(mcu->rx);
	/* The probe may have decided not to register one. */
	if (mcu->chip_added)
		gpiochip_remove(&mcu->gc);

	lines = mcu->n_relays + mcu->n_contacts;
	if (mcu->names) {
		for (i = 0; i < lines; i++)
			kfree(mcu->names[i]);
		kfree(mcu->names);
	}

	tty->disc_data = NULL;
	kfree(mcu);
	pr_info("detached from %s\n", tty->name);
}

/*
 * receive_buf2, NOT receive_buf. The fallback path clamps to
 * tty->receive_room, which is zero unless the discipline sets it — so a
 * discipline with only receive_buf is handed nothing, forever, with no error
 * anywhere: writes succeed, the far end answers, every request times out.
 * receive_buf2 is not clamped. receive_room is set as well, for the fallback.
 */
static size_t ohc_ldisc_receive_buf2(struct tty_struct *tty, const u8 *cp,
				     const u8 *fp, size_t count)
{
	struct ohc_iomcu *mcu = tty->disc_data;

	if (mcu)
		ohc_feed(mcu, cp, count);
	return count;
}

static void ohc_ldisc_receive_buf(struct tty_struct *tty, const u8 *cp,
				  const u8 *fp, size_t count)
{
	struct ohc_iomcu *mcu = tty->disc_data;

	if (mcu)
		ohc_feed(mcu, cp, count);
}

static ssize_t ohc_ldisc_read(struct tty_struct *tty, struct file *file,
			      u8 *buf, size_t nr, void **cookie,
			      unsigned long offset)
{
	/* The bytes belong to the gpiochip, not to whoever holds the fd. */
	return -EIO;
}

static ssize_t ohc_ldisc_write(struct tty_struct *tty, struct file *file,
			       const u8 *buf, size_t nr)
{
	return -EIO;
}

static struct tty_ldisc_ops ohc_ldisc = {
	.owner		= THIS_MODULE,
	.name		= "ohc_iomcu",
	.open		= ohc_ldisc_open,
	.close		= ohc_ldisc_close,
	.read		= ohc_ldisc_read,
	.write		= ohc_ldisc_write,
	.receive_buf	= ohc_ldisc_receive_buf,
	.receive_buf2	= ohc_ldisc_receive_buf2,
};

static int __init ohc_iomcu_init(void)
{
	int ret;

	ohc_ldisc.num = ldisc_num;
	ret = tty_register_ldisc(&ohc_ldisc);
	if (ret) {
		pr_err("cannot register line discipline %d: %d\n", ldisc_num, ret);
		return ret;
	}
	pr_info("line discipline %d registered\n", ldisc_num);
	return 0;
}

static void __exit ohc_iomcu_exit(void)
{
	tty_unregister_ldisc(&ohc_ldisc);
}

module_init(ohc_iomcu_init);
module_exit(ohc_iomcu_exit);

MODULE_DESCRIPTION("Control4 IO microcontroller relays and contacts as a GPIO chip");
MODULE_AUTHOR("Joseph Telaak");
MODULE_LICENSE("GPL");
