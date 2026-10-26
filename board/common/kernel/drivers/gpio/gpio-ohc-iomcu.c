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
 * TWO FRONT ENDS, one core. Everything below the transport — framing, the
 * bootloader bring-up, the gpiochip, the lirc devices, the MCU-routed ttys — is
 * shared, and each front end only supplies a way to write bytes and feeds what
 * it reads into ohc_feed().
 *
 *   serdev        A board whose device tree puts a "control4,io-mcu" node under
 *                 the MCU's UART (the EA family, from its built-in DTB) gets
 *                 the device bound at boot, counts and line rate taken from the
 *                 node. Nothing in userspace is involved.
 *
 *   line          A board with no tree (the HC-800's BIOS has no node for a
 *   discipline    Control4 co-processor) has nothing for serdev to enumerate.
 *                 TIOCSETD needs no enumeration — the way PPP, SLIP and n_gsm
 *                 attach — so S12iomcu writes the counts into the module
 *                 parameters and iomcu-attach hands the port over.
 *
 * Copyright (C) 2026 openHC
 */

#define pr_fmt(fmt) "ohc-iomcu: " fmt

#include <linux/cdev.h>
#include <linux/completion.h>
#include <linux/delay.h>
#include <linux/device.h>
#include <linux/fs.h>
#include <linux/gpio/driver.h>
#include <linux/math64.h>
#include <media/rc-core.h>
#include <linux/mod_devicetable.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/printk.h>
#include <linux/property.h>
#include <linux/serdev.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/tty.h>
#include <linux/tty_driver.h>
#include <linux/tty_flip.h>
#include <linux/tty_ldisc.h>
#include <linux/tty_port.h>
#include <linux/uaccess.h>
#include <linux/wait.h>
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
 * The line discipline's geometry. A serdev device ignores these and reads the
 * same counts from its device tree node instead.
 *
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

/*
 * MCU-routed user serial ports — the combo jacks when switched to serial. Each
 * becomes a /dev/ttyOHC<n> tty whose bytes travel as UART_SEND / UART_RECEIVE
 * frames over the same port as the relays and IR. Zero means this board routes
 * no serial through the microcontroller.
 */
static int uarts;
module_param(uarts, int, 0644);
MODULE_PARM_DESC(uarts, "number of MCU-routed user serial ports (combo jacks in serial mode)");

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
/*
 * User-serial opcodes, verified on a live EA3. All three are UNACKNOWLEDGED —
 * UART_SET_CONTROL and UART_SEND get no reply, and received bytes arrive
 * asynchronously as UART_RECEIVE — which is why the serial path uses ohc_send
 * (fire-and-forget) throughout and never ohc_request.
 */
#define OP_UART_SET_CONTROL 0xa0
#define OP_UART_SEND        0xa1
#define OP_UART_RECEIVE     0xa2

#define OHC_MAX_PAYLOAD 512
#define OHC_TIMEOUT_MS  600

/*
 * UART_SET_CONTROL takes a baud INDEX, not a rate. 0..6 are the rates below;
 * anything else the firmware treats as 115200, and 7 is the first such value.
 * One UART_SEND carries [port, data...]; it is chunked so a long write cannot
 * overrun one frame.
 */
#define OHC_UART_TX_CHUNK 256
static const u32 ohc_uart_baud_table[] = {
	1200, 2400, 4800, 9600, 19200, 38400, 57600,
};
static u8 ohc_uart_baud_index(u32 baud)
{
	int i;

	for (i = 0; i < (int)ARRAY_SIZE(ohc_uart_baud_table); i++)
		if (ohc_uart_baud_table[i] == baud)
			return (u8)i;
	return 7;	/* firmware's "anything else" case: 115200 */
}

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

/*
 * One MCU-routed user serial port: a tty whose bytes travel as UART_SEND /
 * UART_RECEIVE frames over the same ttyS1 the gpiochip and IR use.
 */
struct ohc_uart {
	struct tty_port port;
	struct ohc_iomcu *mcu;
	int index;		/* 0-based port selector on the wire */
	u32 baud;
	u8 data_bits;
	u8 parity;		/* 0 none, 1 odd, 2 even */
	u8 stop_bits;
};

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

/*
 * What sits behind the part, from the module parameters (line discipline) or
 * the device tree node (serdev). Latched once, at attach or probe.
 */
struct ohc_geometry {
	int relays;
	int contacts;
	int ir_out;		/* rear jacks */
	int ir_blaster;		/* 1 = internal front blaster */
	int ir_in;		/* 1 = front receiver */
	int uarts;		/* MCU-routed user serial ports */
};

struct ohc_iomcu {
	/*
	 * The transport, set by the front end. write() returns the bytes it
	 * took or -errno; the front end's receive path calls ohc_feed().
	 */
	int (*write)(struct ohc_iomcu *mcu, const u8 *buf, int len);
	const char *name;		/* for messages: ttyS3, serial0-0 */
	struct tty_struct *tty;		/* line discipline only */
	struct serdev_device *serdev;	/* serdev only */

	struct ohc_geometry geo;
	struct gpio_chip gc;
	struct device *dev;		/* parent for the chip and rc devices */

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

	/*
	 * Bootloader bring-up. The TI serial bootloader does not speak DLE/STX,
	 * so while raw is set received bytes collect here instead of reaching
	 * the decoder. See ohc_bringup.
	 */
	bool raw;
	u8 raw_buf[64];
	int raw_len;
	wait_queue_head_t raw_wait;

	u8 seq;

	/*
	 * Contact state, refreshed by the poller. Relays are deliberately NOT
	 * cached: they are read rarely and a stale relay reading is a worse
	 * thing to hand out than a slow one.
	 */
	u32 contact_mask;
	bool contacts_valid;

	/*
	 * The /dev/ohc view. See ohc_add_nodes.
	 */
	dev_t devt;
	struct cdev cdev;
	bool nodes_added;

	struct delayed_work poll_work;
	/*
	 * Identify and register the chip AFTER open() returns. See ohc_probe.
	 */
	struct delayed_work probe_work;
	bool chip_added;
	const char **names;

	/*
	 * /sys/class/ohc/iomcu: what this part is declared to have, published at
	 * attach — before it answers, so a silent part still says what it should
	 * have been. See ohc_core_start.
	 */
	struct device *info;
	u32 baud;
	bool online;		/* answered FIRMWARE_VERSION */

	/*
	 * Latched at attach, from geo. Reading the module parameters on every
	 * access would let a sysfs write shift the line map underneath a
	 * gpiochip that has already told userspace what its lines are.
	 */
	int n_relays;
	int n_contacts;

	/* One lirc device per emitter; the receiver is separate, because it is
	 * the only one that should present an input device. */
	struct ohc_emitter *emitters;
	int n_emitters;
	struct rc_dev *rx;

	/*
	 * MCU-routed user serial ports, exposed as /dev/ttyOHC<n>. A port is
	 * switched to serial mode (UART_SET_CONTROL) only when its tty is
	 * opened, so the IR emitter sharing the same combo jack keeps working
	 * until somebody actually asks for the console.
	 */
	struct tty_driver *serial_driver;
	struct ohc_uart *uarts;
	int n_uarts;

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

/* Hand bytes to the front end's transport, and count them for the diagnosis. */
static int ohc_xmit(struct ohc_iomcu *mcu, const u8 *buf, int len)
{
	int ret = mcu->write(mcu, buf, len);

	if (ret > 0)
		mcu->tx_bytes += ret;
	if (ret >= 0 && ret < len)
		pr_warn("%s: short write: %d of %d bytes\n", mcu->name, ret, len);
	return ret < 0 ? ret : 0;
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

	ret = ohc_xmit(mcu, out, n);
	kfree(out);
	return ret;
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
		/* div_u64, not `/`. A 64-bit divide compiles to a libgcc helper
		 * (__aeabi_uldivmod on ARM) that the kernel does not link against,
		 * so a plain `/` here builds on x86 and fails at `LD vmlinux` on
		 * every 32-bit board. The multiply genuinely needs 64 bits —
		 * 0xffff * 500000 overflows u32 — so widening is not the fix.
		 */
		u32 periods = div_u64((u64)txbuf[i] * carrier, 1000000u);

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
		ev.duration = div_u64((u64)(w & 0x7fff) * 1000000u, carrier);
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

static void ohc_uart_rx(struct ohc_iomcu *mcu, const u8 *payload, int len);

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
	 * Unsolicited: a remote was pressed at the front receiver, or a byte
	 * arrived on one of the MCU-routed serial ports. This is the whole
	 * reason the decoder keeps frames it did not ask for.
	 */
	if (opcode == OP_IRIN_CAPTURED)
		ohc_ir_capture(mcu, mcu->body + 5, len);
	else if (opcode == OP_UART_RECEIVE)
		ohc_uart_rx(mcu, mcu->body + 5, len);
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

	if (READ_ONCE(mcu->raw)) {
		unsigned long flags;
		int take;

		spin_lock_irqsave(&mcu->rx_lock, flags);
		take = min_t(int, count, (int)sizeof(mcu->raw_buf) - mcu->raw_len);
		memcpy(mcu->raw_buf + mcu->raw_len, buf, take);
		mcu->raw_len += take;
		spin_unlock_irqrestore(&mcu->rx_lock, flags);
		wake_up(&mcu->raw_wait);
		return;
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

/* ---- /dev/ohc/relay1 ------------------------------------------------------
 *
 * A file per line, so `echo 1 > /dev/ohc/relay1` works and somebody who has
 * never met libgpiod can still close a relay.
 *
 * This is a VIEW, not a second implementation. Every access goes through the
 * same ohc_relay_get/toggle the gpiochip uses, behind the same io_lock, and it
 * requests NO gpio descriptor — so `gpioset` and this cannot lock each other
 * out, and the gpiochip remains the authoritative interface. Nothing here knows
 * anything about the wire protocol that the gpiochip path does not.
 *
 * Why bother, when the lines are already named: libgpiod 1.x is what Buildroot
 * ships (2024.02.9 and 2025.02 both pin 1.6.x), and its tools cannot address a
 * line by name — the best it offers is `gpioset $(gpiofind relay1)=1`. libgpiod
 * 2 fixes that with `gpioset relay1=1`, and when the fleet gets there this file
 * becomes a convenience rather than the ergonomic answer. It is still the
 * interface a shell script, a cron job or a curious installer reaches for
 * first, which is the whole argument for it.
 *
 * The vendor shipped this shape too — /dev/gpio/dsp_reset on a stock EA — but
 * as SYMLINKS, not a driver: their /etc/init.d/gpio does
 * `ln -s /sys/class/gpio/gpioN/value /dev/gpio/<name>` over the deprecated GPIO
 * sysfs, and `ln -s /sys/class/leds/<led>/brightness` for the LEDs. That ABI is
 * gone from modern kernels, and it carried the export/ownership hole that got
 * it deprecated. Same ergonomics here, over the chardev-era gpiolib instead.
 */
/* Minors are allocated contiguously from mcu->devt, which is not required to
 * start at 0 — so the line index is the offset from that base, not the raw
 * minor. */
static int ohc_node_index(struct ohc_iomcu *mcu, struct file *f)
{
	return iminor(file_inode(f)) - MINOR(mcu->devt);
}

static ssize_t ohc_node_read(struct file *f, char __user *buf, size_t len,
			     loff_t *ppos)
{
	struct ohc_iomcu *mcu = f->private_data;
	int idx = ohc_node_index(mcu, f);
	char out[3];
	bool on;
	int ret;

	if (idx < mcu->n_relays) {
		ret = ohc_relay_get(mcu, idx, &on);
		if (ret)
			return ret;
	} else {
		u32 mask;

		ret = ohc_contacts_get(mcu, &mask);
		if (ret)
			return ret;
		on = !!(mask & BIT(idx - mcu->n_relays));
	}
	out[0] = on ? '1' : '0';
	out[1] = '\n';
	out[2] = '\0';
	return simple_read_from_buffer(buf, len, ppos, out, 2);
}

/*
 * Accepts what a person would type: 0/1, on/off, true/false, high/low. A
 * contact is an input and says so with -EPERM rather than silently doing
 * nothing.
 */
static ssize_t ohc_node_write(struct file *f, const char __user *buf,
			      size_t len, loff_t *ppos)
{
	struct ohc_iomcu *mcu = f->private_data;
	int idx = ohc_node_index(mcu, f);
	char in[16];
	bool want, now;
	int ret;

	if (idx >= mcu->n_relays)
		return -EPERM;
	if (len == 0 || len >= sizeof(in))
		return -EINVAL;
	if (copy_from_user(in, buf, len))
		return -EFAULT;
	in[len] = '\0';
	strim(in);

	if (!strcmp(in, "1") || !strcasecmp(in, "on") ||
	    !strcasecmp(in, "true") || !strcasecmp(in, "high"))
		want = true;
	else if (!strcmp(in, "0") || !strcasecmp(in, "off") ||
		 !strcasecmp(in, "false") || !strcasecmp(in, "low"))
		want = false;
	else
		return -EINVAL;

	/* Idempotent, for the same reason ohc_gpio_set is: the firmware has no
	 * SET opcode, and a toggle-only write would invert a relay every time
	 * something wrote the value it already had. */
	ret = ohc_relay_get(mcu, idx, &now);
	if (ret)
		return ret;
	if (now != want) {
		ret = ohc_relay_toggle(mcu, idx, &now);
		if (ret)
			return ret;
		if (now != want)
			return -EIO;
	}
	return len;
}

static int ohc_node_open(struct inode *ino, struct file *f)
{
	f->private_data = container_of(ino->i_cdev, struct ohc_iomcu, cdev);
	return 0;
}

static const struct file_operations ohc_node_fops = {
	.owner   = THIS_MODULE,
	.open    = ohc_node_open,
	.read    = ohc_node_read,
	.write   = ohc_node_write,
	.llseek  = default_llseek,
};

/* Put them under /dev/ohc/ the way DRM and ALSA place theirs, rather than
 * scattering relay1..N across the root of /dev. */
static char *ohc_devnode(const struct device *dev, umode_t *mode)
{
	return kasprintf(GFP_KERNEL, "ohc/%s", dev_name(dev));
}

static struct class ohc_class = {
	.name    = "ohc",
	.devnode = ohc_devnode,
};

static void ohc_add_nodes(struct ohc_iomcu *mcu, int lines)
{
	int i;

	if (alloc_chrdev_region(&mcu->devt, 0, lines, "ohc"))
		return;

	cdev_init(&mcu->cdev, &ohc_node_fops);
	mcu->cdev.owner = THIS_MODULE;
	if (cdev_add(&mcu->cdev, mcu->devt, lines)) {
		unregister_chrdev_region(mcu->devt, lines);
		return;
	}

	for (i = 0; i < lines; i++)
		device_create(&ohc_class, NULL, mcu->devt + i,
			      NULL, "%s", mcu->names[i]);
	mcu->nodes_added = true;
	pr_info("%s: /dev/ohc/%s .. %s\n", mcu->name,
		mcu->names[0], mcu->names[lines - 1]);
}

static void ohc_del_nodes(struct ohc_iomcu *mcu, int lines)
{
	int i;

	if (!mcu->nodes_added)
		return;
	for (i = 0; i < lines; i++)
		device_destroy(&ohc_class, mcu->devt + i);
	cdev_del(&mcu->cdev);
	unregister_chrdev_region(mcu->devt, lines);
	mcu->nodes_added = false;
}

/* ---- MCU-routed user serial --------------------------------------------- *
 *
 * A combo jack in serial mode is a tty like any other: iod's host-UART bridge
 * opens /dev/ttyOHC<n> and never learns that its bytes are UART_SEND frames
 * multiplexed onto the same ttyS1 as the relays and the IR. This is the n_gsm
 * pattern — a line discipline that also offers ttys of its own.
 *
 * The port is switched to serial (UART_SET_CONTROL) on OPEN, not at attach, so
 * the IR emitter that shares the connector keeps working until a console is
 * actually asked for. Every UART opcode is unacknowledged, so TX goes through
 * ohc_send rather than ohc_request.
 */

/* Push the current line settings to the part. Fire-and-forget. */
static int ohc_uart_apply(struct ohc_uart *u)
{
	u8 pl[6];

	pl[0] = (u8)u->index;
	pl[1] = 1;			/* enable / serial mode */
	pl[2] = ohc_uart_baud_index(u->baud);
	pl[3] = u->data_bits;
	pl[4] = u->parity;
	pl[5] = u->stop_bits;
	return ohc_send(u->mcu, OP_UART_SET_CONTROL, pl, sizeof(pl));
}

static int ohc_serial_activate(struct tty_port *port, struct tty_struct *tty)
{
	struct ohc_uart *u = container_of(port, struct ohc_uart, port);

	return ohc_uart_apply(u);
}

/*
 * No modem lines on a 3.5 mm jack, so carrier is always present: a blocking
 * open must not hang waiting for a DCD that can never assert.
 */
static bool ohc_serial_carrier_raised(struct tty_port *port)
{
	return true;
}

static const struct tty_port_operations ohc_serial_port_ops = {
	.activate	= ohc_serial_activate,
	.carrier_raised	= ohc_serial_carrier_raised,
};

static int ohc_serial_install(struct tty_driver *driver, struct tty_struct *tty)
{
	struct ohc_iomcu *mcu = driver->driver_state;
	struct ohc_uart *u;

	if (tty->index < 0 || tty->index >= mcu->n_uarts)
		return -ENODEV;
	u = &mcu->uarts[tty->index];
	tty->driver_data = u;
	return tty_port_install(&u->port, driver, tty);
}

static int ohc_serial_open(struct tty_struct *tty, struct file *filp)
{
	struct ohc_uart *u = tty->driver_data;

	return tty_port_open(&u->port, tty, filp);
}

static void ohc_serial_close(struct tty_struct *tty, struct file *filp)
{
	struct ohc_uart *u = tty->driver_data;

	tty_port_close(&u->port, tty, filp);
}

static void ohc_serial_hangup(struct tty_struct *tty)
{
	struct ohc_uart *u = tty->driver_data;

	tty_port_hangup(&u->port);
}

static ssize_t ohc_serial_write(struct tty_struct *tty, const u8 *buf, size_t count)
{
	struct ohc_uart *u = tty->driver_data;
	size_t done = 0;
	u8 pl[1 + OHC_UART_TX_CHUNK];

	/*
	 * Each chunk becomes one UART_SEND [port, data...]. A short write (the
	 * lower tty refusing bytes) stops here and the line discipline retries
	 * the rest, exactly as it would for any tty.
	 */
	while (done < count) {
		size_t n = min(count - done, (size_t)OHC_UART_TX_CHUNK);

		pl[0] = (u8)u->index;
		memcpy(pl + 1, buf + done, n);
		if (ohc_send(u->mcu, OP_UART_SEND, pl, n + 1))
			break;
		done += n;
	}
	return done;
}

static unsigned int ohc_serial_write_room(struct tty_struct *tty)
{
	/* No queue of our own: each write turns straight into UART_SEND frames,
	 * so report a chunk to keep the line discipline handing bytes down in
	 * sane units. */
	return OHC_UART_TX_CHUNK;
}

static void ohc_serial_set_termios(struct tty_struct *tty,
				   const struct ktermios *old)
{
	struct ohc_uart *u = tty->driver_data;
	tcflag_t c = tty->termios.c_cflag;

	u->baud = tty_get_baud_rate(tty);
	switch (c & CSIZE) {
	case CS5: u->data_bits = 5; break;
	case CS6: u->data_bits = 6; break;
	case CS7: u->data_bits = 7; break;
	default:  u->data_bits = 8; break;
	}
	u->parity = (c & PARENB) ? ((c & PARODD) ? 1 : 2) : 0;
	u->stop_bits = (c & CSTOPB) ? 2 : 1;
	ohc_uart_apply(u);
}

static const struct tty_operations ohc_serial_tty_ops = {
	.install	= ohc_serial_install,
	.open		= ohc_serial_open,
	.close		= ohc_serial_close,
	.hangup		= ohc_serial_hangup,
	.write		= ohc_serial_write,
	.write_room	= ohc_serial_write_room,
	.set_termios	= ohc_serial_set_termios,
};

/*
 * Deliver bytes that arrived as a UART_RECEIVE (0xa2) frame to the matching
 * tty. Called from the decoder — the tty buffer worker — which is the context
 * tty_insert_flip_string is built for. payload is [port, data...].
 */
static void ohc_uart_rx(struct ohc_iomcu *mcu, const u8 *payload, int len)
{
	struct ohc_uart *u;

	if (len < 1 || payload[0] >= (u8)mcu->n_uarts)
		return;
	u = &mcu->uarts[payload[0]];
	if (len > 1) {
		tty_insert_flip_string(&u->port, payload + 1, len - 1);
		tty_flip_buffer_push(&u->port);
	}
}

static void ohc_register_serial(struct ohc_iomcu *mcu)
{
	struct tty_driver *drv;
	int n = mcu->geo.uarts;
	int i, ret;

	if (n <= 0)
		return;
	if (n > 32) {
		pr_warn("%s: %d user serial ports is implausible; ignoring\n",
			mcu->name, n);
		return;
	}

	mcu->uarts = kcalloc(n, sizeof(*mcu->uarts), GFP_KERNEL);
	if (!mcu->uarts)
		return;

	drv = tty_alloc_driver(n, TTY_DRIVER_REAL_RAW | TTY_DRIVER_DYNAMIC_DEV);
	if (IS_ERR(drv)) {
		kfree(mcu->uarts);
		mcu->uarts = NULL;
		return;
	}

	drv->driver_name = "ohc-iomcu-serial";
	drv->name = "ttyOHC";
	drv->type = TTY_DRIVER_TYPE_SERIAL;
	drv->init_termios = tty_std_termios;
	drv->init_termios.c_cflag = B115200 | CS8 | CREAD | CLOCAL | HUPCL;
	drv->init_termios.c_ispeed = 115200;
	drv->init_termios.c_ospeed = 115200;
	tty_set_operations(drv, &ohc_serial_tty_ops);
	drv->driver_state = mcu;

	for (i = 0; i < n; i++) {
		struct ohc_uart *u = &mcu->uarts[i];

		tty_port_init(&u->port);
		u->port.ops = &ohc_serial_port_ops;
		u->mcu = mcu;
		u->index = i;
		u->baud = 115200;
		u->data_bits = 8;
		u->parity = 0;
		u->stop_bits = 1;
	}

	ret = tty_register_driver(drv);
	if (ret) {
		pr_warn("%s: cannot register serial tty driver: %d\n",
			mcu->name, ret);
		for (i = 0; i < n; i++)
			tty_port_destroy(&mcu->uarts[i].port);
		tty_driver_kref_put(drv);
		kfree(mcu->uarts);
		mcu->uarts = NULL;
		return;
	}

	mcu->serial_driver = drv;
	mcu->n_uarts = n;

	for (i = 0; i < n; i++)
		tty_port_register_device(&mcu->uarts[i].port, drv, i, mcu->dev);

	pr_info("%s: %d MCU-routed user serial port(s) as /dev/%s0..%d\n",
		mcu->name, n, drv->name, n - 1);
}

static void ohc_unregister_serial(struct ohc_iomcu *mcu)
{
	int i;

	if (!mcu->serial_driver)
		return;
	for (i = 0; i < mcu->n_uarts; i++) {
		tty_port_tty_hangup(&mcu->uarts[i].port, false);
		tty_unregister_device(mcu->serial_driver, i);
	}
	tty_unregister_driver(mcu->serial_driver);
	for (i = 0; i < mcu->n_uarts; i++)
		tty_port_destroy(&mcu->uarts[i].port);
	tty_driver_kref_put(mcu->serial_driver);
	kfree(mcu->uarts);
	mcu->uarts = NULL;
	mcu->serial_driver = NULL;
	mcu->n_uarts = 0;
}

/* ---- registration ------------------------------------------------------- */

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
	rc->dev.parent = mcu->dev;
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
	int n = mcu->geo.ir_out + (mcu->geo.ir_blaster ? 1 : 0);
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
		for (i = 0; i < mcu->geo.ir_out; i++) {
			int ret = ohc_register_emitter(mcu, &mcu->emitters[i], 1u << i,
						       kasprintf(GFP_KERNEL,
								 "openHC IR out %d", i + 1));

			if (ret)
				pr_warn("%s: IR out %d did not register: %d\n",
					mcu->name, i + 1, ret);
			else
				ok++;
		}
		if (mcu->geo.ir_blaster) {
			int ret = ohc_register_emitter(mcu, &mcu->emitters[mcu->geo.ir_out],
						       1u << mcu->geo.ir_out,
						       kstrdup("openHC IR front blaster",
							       GFP_KERNEL));

			if (ret)
				pr_warn("%s: front blaster did not register: %d\n",
					mcu->name, ret);
			else
				ok++;
		}
	}

	if (mcu->geo.ir_in) {
		u8 on = 1;

		/*
		 * The only node with an input device: it is the only one that
		 * can produce a key press.
		 */
		rx = rc_allocate_device(RC_DRIVER_IR_RAW);
		if (rx) {
			rx->priv = mcu;
			rx->dev.parent = mcu->dev;
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
		pr_info("%s: %d IR emitter(s)%s\n", mcu->name, ok,
			mcu->rx ? " and a receiver" : "");
}

/* ---- bootloader bring-up ------------------------------------------------ */

static void ohc_raw_reset(struct ohc_iomcu *mcu)
{
	unsigned long flags;

	spin_lock_irqsave(&mcu->rx_lock, flags);
	mcu->raw_len = 0;
	spin_unlock_irqrestore(&mcu->rx_lock, flags);
}

static bool ohc_raw_seen(struct ohc_iomcu *mcu, const u8 *needle, int n)
{
	unsigned long flags;
	bool hit = false;
	int i;

	spin_lock_irqsave(&mcu->rx_lock, flags);
	for (i = 0; !hit && i + n <= mcu->raw_len; i++)
		hit = !memcmp(mcu->raw_buf + i, needle, n);
	spin_unlock_irqrestore(&mcu->rx_lock, flags);
	return hit;
}

/*
 * Send @out on a clean slate, then wait up to @ms for @needle to show up in
 * what comes back. No needle: just give the part @ms to act on it.
 */
static bool ohc_raw_xfer(struct ohc_iomcu *mcu, const u8 *out, int len,
			 const u8 *needle, int n, int ms)
{
	ohc_raw_reset(mcu);
	if (ohc_xmit(mcu, out, len))
		return false;
	if (!needle) {
		msleep(ms);
		return true;
	}
	return wait_event_timeout(mcu->raw_wait, ohc_raw_seen(mcu, needle, n),
				  msecs_to_jiffies(ms)) > 0;
}

/*
 * Start the application. The part (TM4C on EA, LM3S on HC) powers up in the TI
 * serial bootloader and does NOT run its application on its own; until it does
 * it answers nothing on DLE/STX, which was long mistaken for dead silicon. The
 * plain TI sequence, bytes from the vendor's reflash library:
 *
 *   autobaud      55 55                  -> 00 cc  (bootloader ACK)
 *   PING          03 20 20               -> 00 cc
 *   COMMAND_RUN   07 32 22 00 00 10 00   -> nothing, by design: it has jumped
 *                                           to the application at 0x1000
 *   app autobaud  55 55                  -> 10 02 d7 ... (AUTO_BAUD_RESP)
 *
 * The last knock is for the application, which autobauds onto our line rate
 * from its own first 55 55. GET_STATUS is deliberately not sent: its reply is a
 * packet that must itself be ACKed or the bootloader desyncs.
 *
 * Runs from probe_work, never from the ldisc's open(), for the reason given at
 * ohc_probe — the replies have to be able to reach ohc_feed.
 */
static int ohc_bringup(struct ohc_iomcu *mcu)
{
	static const u8 knock[] = { 0x55, 0x55 };
	static const u8 ack[]   = { 0x00, 0xcc };
	static const u8 ping[]  = { 0x03, 0x20, 0x20 };
	static const u8 run[]   = { 0x07, 0x32, 0x22, 0x00, 0x00, 0x10, 0x00 };
	static const u8 dstx[]  = { DLE, STX };
	bool ok;

	mutex_lock(&mcu->io_lock);
	WRITE_ONCE(mcu->raw, true);
	ok = ohc_raw_xfer(mcu, knock, sizeof(knock), ack, sizeof(ack), 400);
	if (ok) {
		/* PING mostly confirms the link before the un-ACKed RUN. */
		ohc_raw_xfer(mcu, ping, sizeof(ping), ack, sizeof(ack), 400);
		ohc_raw_xfer(mcu, run, sizeof(run), NULL, 0, 300);
		ohc_raw_xfer(mcu, knock, sizeof(knock), dstx, sizeof(dstx), 700);
	}
	/* Whatever the decoder held from before is stale; hunt afresh. */
	mcu->state = RX_IDLE;
	mcu->body_len = 0;
	WRITE_ONCE(mcu->raw, false);
	mutex_unlock(&mcu->io_lock);

	if (!ok)
		pr_warn("%s: the TI bootloader did not ACK autobaud (55 55)\n",
			mcu->name);
	return ok ? 0 : -ENODEV;
}

/*
 * Identify the microcontroller, then register the chip.
 *
 * THIS CANNOT HAPPEN IN open(). tty_set_ldisc() holds tty->ldisc_sem across the
 * ldisc's open(), and the receive path needs that same semaphore to deliver the
 * reply — so a request from open() times out every time, and the part looks
 * dead to this driver while answering userspace perfectly. serdev has no such
 * lock, but takes the same deferred path: a second of bring-up has no business
 * in the boot-time probe.
 */
static void ohc_probe(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, probe_work);
	struct ohc_frame f;
	int i, lines, ret;

	/*
	 * Ask the application first, so a part that is already running — a
	 * warm reboot, a kexec, or iomcu-attach having done this — is left
	 * alone; only silence earns the bootloader sequence.
	 */
	ret = ohc_request(mcu, OP_FIRMWARE_VERSION, NULL, 0, &f);
	if (ret) {
		pr_info("%s: no application answering; starting it from the TI bootloader\n",
			mcu->name);
		if (!ohc_bringup(mcu))
			ret = ohc_request(mcu, OP_FIRMWARE_VERSION, NULL, 0, &f);
	}
	if (ret) {
		/*
		 * No chip. A gpiochip standing in for a part that is not
		 * answering is one whose every read is a lie, and it would be
		 * indistinguishable from working hardware until somebody
		 * trusted a contact.
		 */
		pr_warn("no reply from the IO microcontroller on %s; no gpiochip registered\n",
			mcu->name);
		pr_warn("  tx=%lu rx=%lu frames=%lu badcsum=%lu\n",
			mcu->tx_bytes, mcu->rx_bytes, mcu->rx_frames, mcu->rx_bad_csum);
		if (mcu->first_rx_len)
			print_hex_dump(KERN_WARNING, "ohc-iomcu: first rx: ",
				       DUMP_PREFIX_NONE, 16, 1,
				       mcu->first_rx, mcu->first_rx_len, false);
		return;
	}
	WRITE_ONCE(mcu->online, true);

	lines = mcu->n_relays + mcu->n_contacts;
	if (!lines) {
		/* No GPIO lines (the EA1): no gpiochip, but the IR emitters and
		 * the MCU-routed serial ports below are still worth having. */
		ohc_register_serial(mcu);
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
	mcu->gc.parent = mcu->dev;
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

	/* The friendly view of the same lines. Not fatal if it fails. */
	ohc_add_nodes(mcu, lines);

	if (poll_ms > 0)
		schedule_delayed_work(&mcu->poll_work, msecs_to_jiffies(poll_ms));

	pr_info("%s: %d relays, %d contacts as GPIO lines\n",
		mcu->name, mcu->n_relays, mcu->n_contacts);

	ohc_register_serial(mcu);
	ohc_register_rc(mcu);
	return;

err_names:
	for (i = 0; i < lines; i++)
		kfree(mcu->names[i]);
	kfree(mcu->names);
	mcu->names = NULL;
}

/* ---- shared attach / detach --------------------------------------------- */

/*
 * Everything a front end needs before it opens the port. The chip itself is
 * registered later, from probe_work, once the part has answered.
 */
static struct ohc_iomcu *ohc_core_alloc(const struct ohc_geometry *geo)
{
	struct ohc_iomcu *mcu;

	if (geo->relays < 0 || geo->contacts < 0 ||
	    geo->relays > 32 || geo->contacts > 32)
		return ERR_PTR(-EINVAL);
	/*
	 * A board may have IR and no GPIO lines at all (the EA1), so "no relays
	 * and no contacts" is not a reason to refuse the port — only "nothing
	 * at all behind it" is.
	 */
	if (geo->relays + geo->contacts + geo->ir_out + geo->ir_blaster +
	    geo->ir_in + geo->uarts == 0)
		return ERR_PTR(-EINVAL);

	mcu = kzalloc(sizeof(*mcu), GFP_KERNEL);
	if (!mcu)
		return ERR_PTR(-ENOMEM);

	mcu->geo = *geo;
	mcu->n_relays = geo->relays;
	mcu->n_contacts = geo->contacts;
	mutex_init(&mcu->io_lock);
	spin_lock_init(&mcu->rx_lock);
	init_completion(&mcu->reply);
	init_waitqueue_head(&mcu->raw_wait);
	INIT_DELAYED_WORK(&mcu->poll_work, ohc_poll);
	INIT_DELAYED_WORK(&mcu->probe_work, ohc_probe);
	return mcu;
}

/*
 * The geometry, from the kernel rather than a second copy in userspace.
 *
 * Whatever described the part — the device tree node, or the module parameters
 * S12iomcu wrote from board.env on a board with no tree — the driver latched
 * it, and this is where iod reads it back:
 *
 *	/sys/class/ohc/iomcu/{relays,contacts,ir_out,ir_blaster,ir_in,uarts}
 *	/sys/class/ohc/iomcu/{port,baud,online}
 *
 * It appears when the port is handed over, not when the part answers: a box
 * whose microcontroller is silent should still say it has four relays, so the
 * diagnosis is "not answering", not "this board has no IO". online says which.
 */
#define OHC_INFO_INT(field, expr)					\
static ssize_t field##_show(struct device *d, struct device_attribute *a, \
			    char *buf)					\
{									\
	struct ohc_iomcu *mcu = dev_get_drvdata(d);			\
									\
	return sysfs_emit(buf, "%d\n", (int)(expr));			\
}									\
static DEVICE_ATTR_RO(field)

OHC_INFO_INT(relays, mcu->geo.relays);
OHC_INFO_INT(contacts, mcu->geo.contacts);
OHC_INFO_INT(ir_out, mcu->geo.ir_out);
OHC_INFO_INT(ir_blaster, mcu->geo.ir_blaster);
OHC_INFO_INT(ir_in, mcu->geo.ir_in);
OHC_INFO_INT(uarts, mcu->geo.uarts);
OHC_INFO_INT(baud, mcu->baud);
OHC_INFO_INT(online, READ_ONCE(mcu->online));

/* The port it is on: ttyS3 behind the line discipline, serial0-0 on serdev. */
static ssize_t port_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_iomcu *mcu = dev_get_drvdata(d);

	return sysfs_emit(buf, "%s\n", mcu->name);
}
static DEVICE_ATTR_RO(port);

static struct attribute *ohc_info_attrs[] = {
	&dev_attr_relays.attr,
	&dev_attr_contacts.attr,
	&dev_attr_ir_out.attr,
	&dev_attr_ir_blaster.attr,
	&dev_attr_ir_in.attr,
	&dev_attr_uarts.attr,
	&dev_attr_port.attr,
	&dev_attr_baud.attr,
	&dev_attr_online.attr,
	NULL,
};
ATTRIBUTE_GROUPS(ohc_info);

/* The port is open and bytes reach ohc_feed: publish, then talk to the part. */
static void ohc_core_start(struct ohc_iomcu *mcu)
{
	/*
	 * No dev_t, so no /dev node: this is a sysfs directory only. Not
	 * fatal if it fails — the chip still works, iod just cannot see the
	 * declaration (and says so).
	 */
	mcu->info = device_create_with_groups(&ohc_class, mcu->dev, 0, mcu,
					      ohc_info_groups, "iomcu");
	if (IS_ERR(mcu->info)) {
		pr_warn("%s: cannot publish /sys/class/ohc/iomcu: %ld\n",
			mcu->name, PTR_ERR(mcu->info));
		mcu->info = NULL;
	}
	schedule_delayed_work(&mcu->probe_work, msecs_to_jiffies(50));
}

/*
 * Undo everything probe_work registered. The port must still be open: a
 * request in flight needs its reply to finish.
 */
static void ohc_core_teardown(struct ohc_iomcu *mcu)
{
	int i, lines;

	if (mcu->info)
		device_unregister(mcu->info);
	cancel_delayed_work_sync(&mcu->probe_work);
	cancel_delayed_work_sync(&mcu->poll_work);
	ohc_unregister_serial(mcu);
	for (i = 0; i < mcu->n_emitters; i++) {
		if (mcu->emitters[i].rc)
			rc_unregister_device(mcu->emitters[i].rc);
		kfree(mcu->emitters[i].name);
	}
	kfree(mcu->emitters);
	if (mcu->rx)
		rc_unregister_device(mcu->rx);
	lines = mcu->n_relays + mcu->n_contacts;
	ohc_del_nodes(mcu, lines);
	/* The probe may have decided not to register one. */
	if (mcu->chip_added)
		gpiochip_remove(&mcu->gc);

	if (mcu->names) {
		for (i = 0; i < lines; i++)
			kfree(mcu->names[i]);
		kfree(mcu->names);
	}
}

/* ---- front end: line discipline ----------------------------------------- */

static int ohc_ldisc_xmit(struct ohc_iomcu *mcu, const u8 *buf, int len)
{
	return mcu->tty->ops->write(mcu->tty, buf, len);
}

static int ohc_ldisc_open(struct tty_struct *tty)
{
	struct ohc_geometry geo = {
		.relays = relays, .contacts = contacts,
		.ir_out = ir_out, .ir_blaster = ir_blaster, .ir_in = ir_in,
		.uarts = uarts,
	};
	struct ohc_iomcu *mcu;

	if (!tty->ops->write)
		return -EOPNOTSUPP;

	mcu = ohc_core_alloc(&geo);
	if (IS_ERR(mcu))
		return PTR_ERR(mcu);

	mcu->tty = tty;
	mcu->name = tty->name;
	/* iomcu-attach set the line rate before handing the port over. */
	mcu->baud = tty_get_baud_rate(tty);
	mcu->write = ohc_ldisc_xmit;
	tty->disc_data = mcu;
	/* See ohc_ldisc_receive_buf2: zero here means nothing is ever delivered. */
	tty->receive_room = 65536;

	/* Talk to the part once this returns and the ldisc lock is released. */
	ohc_core_start(mcu);
	return 0;
}

static void ohc_ldisc_close(struct tty_struct *tty)
{
	struct ohc_iomcu *mcu = tty->disc_data;

	if (!mcu)
		return;

	ohc_core_teardown(mcu);
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

/* ---- front end: serdev -------------------------------------------------- *
 *
 * The board's device tree describes the part as a child of the UART it is
 * wired to, and the counts that the line discipline takes from module
 * parameters come from the node instead:
 *
 *	&uart1 {
 *		io-mcu {
 *			compatible = "control4,io-mcu";
 *			current-speed = <460800>;
 *			control4,relays = <1>;
 *			control4,contacts = <1>;
 *			control4,ir-outputs = <6>;	// rear jacks
 *			control4,ir-blaster;		// internal front emitter
 *			control4,ir-receiver;		// front learner
 *			control4,uarts = <3>;		// MCU-routed serial
 *		};
 *	};
 *
 * A UART with a serdev child is no longer a tty: the port belongs to this
 * driver from boot, and there is no /dev/ttyS<n> left for anything to attach to.
 */
#if IS_REACHABLE(CONFIG_SERIAL_DEV_BUS)

static int ohc_serdev_xmit(struct ohc_iomcu *mcu, const u8 *buf, int len)
{
	return serdev_device_write(mcu->serdev, buf, len,
				   msecs_to_jiffies(OHC_TIMEOUT_MS));
}

static size_t ohc_serdev_receive_buf(struct serdev_device *serdev,
				     const u8 *buf, size_t count)
{
	ohc_feed(serdev_device_get_drvdata(serdev), buf, count);
	return count;
}

static const struct serdev_device_ops ohc_serdev_ops = {
	.receive_buf	= ohc_serdev_receive_buf,
	.write_wakeup	= serdev_device_write_wakeup,
};

/* An absent count is zero: a board describes only what it has. */
static int ohc_prop_count(struct device *dev, const char *prop)
{
	u32 v = 0;

	device_property_read_u32(dev, prop, &v);
	return v > 32 ? -1 : (int)v;
}

static int ohc_serdev_probe(struct serdev_device *serdev)
{
	struct device *dev = &serdev->dev;
	struct ohc_geometry geo = {
		.relays     = ohc_prop_count(dev, "control4,relays"),
		.contacts   = ohc_prop_count(dev, "control4,contacts"),
		.ir_out     = ohc_prop_count(dev, "control4,ir-outputs"),
		.ir_blaster = device_property_read_bool(dev, "control4,ir-blaster"),
		.ir_in      = device_property_read_bool(dev, "control4,ir-receiver"),
		.uarts      = ohc_prop_count(dev, "control4,uarts"),
	};
	struct ohc_iomcu *mcu;
	u32 speed = 115200;
	unsigned int got;
	int ret;

	device_property_read_u32(dev, "current-speed", &speed);

	if (geo.ir_out < 0 || geo.uarts < 0)
		return dev_err_probe(dev, -EINVAL, "implausible IR or serial count\n");
	mcu = ohc_core_alloc(&geo);
	if (IS_ERR(mcu))
		return dev_err_probe(dev, PTR_ERR(mcu),
				     "node describes no relays, contacts, IR or serial\n");

	mcu->serdev = serdev;
	mcu->dev = dev;
	mcu->name = dev_name(dev);
	mcu->write = ohc_serdev_xmit;
	/* Before open: bytes can arrive the moment the port is up. */
	serdev_device_set_drvdata(serdev, mcu);
	serdev_device_set_client_ops(serdev, &ohc_serdev_ops);

	ret = serdev_device_open(serdev);
	if (ret) {
		kfree(mcu);
		return dev_err_probe(dev, ret, "cannot open the UART\n");
	}

	/*
	 * Raw 8N1 and NO flow control. serdev opens a port with CRTSCTS set,
	 * and nothing drives CTS on these boards, so leaving it would stall
	 * the first byte forever.
	 */
	got = serdev_device_set_baudrate(serdev, speed);
	serdev_device_set_flow_control(serdev, false);
	ret = serdev_device_set_parity(serdev, SERDEV_PARITY_NONE);
	if (ret)
		dev_warn(dev, "cannot clear parity: %d\n", ret);
	if (got != speed)
		dev_warn(dev, "asked for %u baud, the UART gave %u\n", speed, got);
	mcu->baud = got;

	ohc_core_start(mcu);
	dev_info(dev, "IO microcontroller at %u baud\n", got);
	return 0;
}

static void ohc_serdev_remove(struct serdev_device *serdev)
{
	struct ohc_iomcu *mcu = serdev_device_get_drvdata(serdev);

	ohc_core_teardown(mcu);
	serdev_device_close(serdev);
	kfree(mcu);
}

static const struct of_device_id ohc_serdev_of_match[] = {
	{ .compatible = "control4,io-mcu" },
	{ }
};
MODULE_DEVICE_TABLE(of, ohc_serdev_of_match);

static struct serdev_device_driver ohc_serdev_driver = {
	.probe	= ohc_serdev_probe,
	.remove	= ohc_serdev_remove,
	.driver	= {
		.name		= "ohc-iomcu",
		.of_match_table	= ohc_serdev_of_match,
	},
};

static int ohc_serdev_register(void)
{
	return serdev_device_driver_register(&ohc_serdev_driver);
}

static void ohc_serdev_unregister(void)
{
	serdev_device_driver_unregister(&ohc_serdev_driver);
}

#else /* no serdev bus: the line discipline is the only way in */

static int ohc_serdev_register(void) { return 0; }
static void ohc_serdev_unregister(void) { }

#endif

static int __init ohc_iomcu_init(void)
{
	int ret;

	ret = class_register(&ohc_class);
	if (ret) {
		pr_err("cannot register the ohc class: %d\n", ret);
		return ret;
	}

	ohc_ldisc.num = ldisc_num;
	ret = tty_register_ldisc(&ohc_ldisc);
	if (ret) {
		pr_err("cannot register line discipline %d: %d\n", ldisc_num, ret);
		class_unregister(&ohc_class);
		return ret;
	}
	pr_info("line discipline %d registered\n", ldisc_num);

	ret = ohc_serdev_register();
	if (ret) {
		pr_err("cannot register the serdev driver: %d\n", ret);
		tty_unregister_ldisc(&ohc_ldisc);
		class_unregister(&ohc_class);
		return ret;
	}
	return 0;
}

static void __exit ohc_iomcu_exit(void)
{
	ohc_serdev_unregister();
	tty_unregister_ldisc(&ohc_ldisc);
	class_unregister(&ohc_class);
}

module_init(ohc_iomcu_init);
module_exit(ohc_iomcu_exit);

MODULE_DESCRIPTION("Control4 IO microcontroller relays and contacts as a GPIO chip");
MODULE_AUTHOR("Joseph Telaak");
MODULE_LICENSE("GPL");
