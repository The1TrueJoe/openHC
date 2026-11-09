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
 * A SERDEV DRIVER, bound by firmware. The board describes the part as a
 * "control4,io-mcu" child of the UART it is wired to, with what sits behind it
 * — in its device tree on the EA family, in an ACPI table (PRP0001, the same
 * properties in _DSD) on the HC-800 — and the driver binds it at boot, starts
 * its application from the TI bootloader if it has to, and registers the chip,
 * the lirc devices and the MCU-routed ttys. Nothing in userspace attaches
 * anything, and there is no second description of the board for it to read.
 *
 * Copyright (C) 2026 openHC
 */

#define pr_fmt(fmt) "ohc-iomcu: " fmt

#include <linux/cdev.h>
#include <linux/completion.h>
#include <linux/crc32.h>
#include <linux/delay.h>
#include <linux/device.h>
#include <linux/firmware.h>
#include <linux/fs.h>
#include <linux/gpio/consumer.h>
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
#include <linux/tty_port.h>
#include <linux/uaccess.h>
#include <linux/unaligned.h>
#include <linux/wait.h>
#include <linux/workqueue.h>

static int poll_ms = 200;
module_param(poll_ms, int, 0644);
MODULE_PARM_DESC(poll_ms, "contact poll interval in milliseconds (0 disables)");

/*
 * The wire protocol, the config frame, the image header and the TI bootloader
 * commands, shared with openHC's firmware (board/common/firmware/io-mcu).
 */
#include "ohc-iomcu-proto.h"

#define DLE OHC_DLE
#define STX OHC_STX

/* Largest payload this side sends or keeps: a 249-burst IR code. */
#define OHC_MAX_PAYLOAD_DRV 512
#define OHC_TIMEOUT_MS  600

/*
 * UART_SET_CONTROL takes a baud INDEX, not a rate. 0..6 are the rates below;
 * anything else the firmware treats as 115200, and 7 is the first such value.
 * One UART_SEND carries [port, data...]; it is chunked so a long write cannot
 * overrun one frame. All three UART opcodes are unacknowledged, so the serial
 * path uses ohc_send throughout.
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
 * IROUT_SEND: 14 fixed bytes then one u16 per burst, in carrier periods. The
 * output is a 24-bit bitmask in the frame, so one lirc device per output
 * with its bit fixed, rather than s_tx_mask. The carrier is a Pronto word the
 * stock firmware DIVIDES BY: zero hard-faults it until a power cycle. Never 0.
 */
#define IR_MAX_DURATIONS ((OHC_MAX_PAYLOAD_DRV - OHC_IR_FIXED) / 2)

enum rx_state {
	RX_IDLE,	/* hunting for DLE */
	RX_DLE,		/* saw DLE, expecting STX */
	RX_BODY,	/* collecting body bytes */
	RX_BODY_DLE,	/* saw DLE inside the body: stuffed, or a new frame */
};

struct ohc_iomcu;
struct ohc_geometry;
static int ohc_blaster_bit(const struct ohc_geometry *geo);

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
	u32 jack;		/* panel jack it shares with an IR output, 0 = none */
	const char *label;
	struct ohc_emitter *ir;	/* that IR output */
	bool serial;		/* the jack's mode; always true without an IR output */
};

/* One IR output: a lirc device that always drives its own bit. */
struct ohc_emitter {
	struct rc_dev *rc;
	struct ohc_iomcu *mcu;
	u32 mask;		/* the bit this node owns */
	u32 carrier;		/* Hz, per device */
	char *name;
	struct ohc_uart *uart;	/* the serial port on the same jack, if any */
};

/*
 * One rear jack from the firmware node: an IR output, an MCU UART, or both
 * (a combo jack, IR or serial at any one time).
 */
/* An MCU pin from the binding: <port pin>, port A..H = 0..7. */
struct ohc_pin {
	bool set;
	u8 port, pin;
};

/* A line routing shared carrier @carrier onto an output: control4,ir-gates. */
struct ohc_gate {
	struct ohc_pin pin;
	u8 carrier;
};

#define OHC_MAX_GATES 4

struct ohc_jack {
	u32 num;		/* panel number */
	const char *label;
	int ir;			/* IR channel (IROUT_SEND bit), -1 = none */
	int uart;		/* MCU user UART, -1 = none */
	struct ohc_pin ir_pin;	/* control4,ir-pin: its own carrier output */
	struct ohc_gate gate[OHC_MAX_GATES];
	int n_gates;
	struct ohc_pin rx, tx;	/* control4,uart-pins */
	struct ohc_pin mux_a, mux_b;	/* control4,uart-mux-pins: IR/serial lines */
};

/* A relay or contact node: its pin, if given, and OHC_CFG_F_* flags. */
struct ohc_line {
	bool seen;
	struct ohc_pin pin;
	u8 flags;
};

struct ohc_frame {
	u8 opcode;
	u8 seq;
	u8 payload[OHC_MAX_PAYLOAD_DRV];
	int len;
};

/*
 * What sits behind the part, from its firmware node. Latched once, at probe.
 */
struct ohc_geometry {
	int relays;
	int contacts;
	int ir_out;		/* rear jacks */
	int ir_blaster;		/* 1 = internal front blaster */
	int ir_in;		/* 1 = front receiver */
	int uarts;		/* MCU-routed user serial ports */
	struct ohc_jack *jacks;
	int n_jacks;
	int blaster_ch;		/* its IROUT_SEND bit, -1 = after the last jack */
	struct ohc_pin blaster_pin;
	struct ohc_gate blaster_gate[OHC_MAX_GATES];
	int n_blaster_gates;
	struct ohc_pin carrier[OHC_CFG_CARRIERS];	/* control4,ir-carriers */
	bool gates_active_low;
	struct ohc_pin hold[16];	/* control4,hold-low-pins */
	int n_hold;
	struct ohc_pin heartbeat;
	struct ohc_pin rx_pin;	/* the receiver */
	u8 rx_flags;
	struct ohc_line *relay;	/* [relays], by wire index */
	struct ohc_line *contact;
};

/* The firmware image the loader writes: its header, then flash bytes. */
struct ohc_img {
	u32 part, load, size;
	char version[OHC_IMG_VERSION_LEN];
	const u8 *data;
};

struct ohc_iomcu {
	struct serdev_device *serdev;	/* the transport; receive calls ohc_feed() */
	const char *name;		/* for messages: serial0-0 */

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
	u8 body[OHC_MAX_PAYLOAD_DRV + 8];
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
	/* Bring-up, identify, register: deferred from probe. See ohc_identify. */
	struct delayed_work identify_work;
	bool chip_added;
	const char **names;

	/*
	 * /sys/class/ohc/iomcu: what this part is declared to have, published at
	 * probe — before it answers, so a silent part still says what it should
	 * have been. See ohc_info_attrs.
	 */
	struct device *info;
	u32 baud;
	bool online;		/* answered FIRMWARE_VERSION */
	bool registered;	/* chip, lirc and ttys are up */
	char fw_version[OHC_IMG_VERSION_LEN];	/* what FIRMWARE_VERSION said */

	/*
	 * The loader. part comes from the compatible, fw_name from
	 * firmware-name; without fw_name nothing is ever written to the part.
	 * in_boot: the bootloader answered and nothing started. updating: a
	 * write is in progress and every request is refused.
	 */
	u32 part;
	const char *fw_name;
	struct gpio_desc *reset;
	bool in_boot;
	bool updating;
	bool fw_cancel;
	int fw_tries;
	struct mutex fw_lock;
	struct delayed_work fw_work;
	struct fw_upload *fwl;
	struct ohc_img upload;

	/* One lirc device per emitter; the receiver is separate, because it is
	 * the only one that should present an input device. */
	struct ohc_emitter *emitters;
	int n_emitters;
	struct rc_dev *rx;
	struct delayed_work rx_idle;

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

/* Hand bytes to the UART, and count them for the diagnosis. */
static int ohc_xmit(struct ohc_iomcu *mcu, const u8 *buf, int len)
{
	int ret = serdev_device_write(mcu->serdev, buf, len,
				      msecs_to_jiffies(OHC_TIMEOUT_MS));

	if (ret > 0)
		mcu->tx_bytes += ret;
	if (ret >= 0 && ret < len)
		pr_warn("%s: short write: %d of %d bytes\n", mcu->name, ret, len);
	return ret < 0 ? ret : 0;
}

static int ohc_write_frame(struct ohc_iomcu *mcu, u8 opcode, u8 seq,
			   const u8 *payload, int len)
{
	u8 hdr[OHC_HDR_LEN] = { opcode, seq, 0, (u8)(len >> 8), (u8)len };
	u8 *out, sum = 0;
	int n = 0, i, ret;

	if (len > OHC_MAX_PAYLOAD_DRV)
		return -EINVAL;

	/* Worst case every byte is DLE and gets stuffed. */
	out = kmalloc(2 + (OHC_HDR_LEN + len + 1) * 2, GFP_KERNEL);
	if (!out)
		return -ENOMEM;

	out[n++] = DLE;
	out[n++] = STX;
	for (i = 0; i < OHC_HDR_LEN + len + 1; i++) {
		u8 b;

		if (i < OHC_HDR_LEN)
			b = hdr[i];
		else if (i < OHC_HDR_LEN + len)
			b = payload[i - OHC_HDR_LEN];
		else
			b = (u8)-sum;	/* negated 8-bit sum */
		sum += b;
		out[n++] = b;
		if (b == DLE)
			out[n++] = DLE;
	}

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

	if (READ_ONCE(mcu->updating) || READ_ONCE(mcu->in_boot))
		return -EBUSY;
	mutex_lock(&mcu->io_lock);
	ret = ohc_write_frame(mcu, opcode, ++mcu->seq, payload, len);
	mutex_unlock(&mcu->io_lock);
	return ret;
}

/* Ask, and wait for the answer with the matching sequence number. */
static int __ohc_request(struct ohc_iomcu *mcu, u8 opcode, const u8 *payload,
			 int len, struct ohc_frame *out, int ms)
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
		&mcu->reply, msecs_to_jiffies(ms));
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
 * Everything but the loader goes through here: nothing while it writes, and
 * never a frame to the bootloader, which would leave it deaf.
 */
static int ohc_request(struct ohc_iomcu *mcu, u8 opcode, const u8 *payload,
		       int len, struct ohc_frame *out)
{
	if (READ_ONCE(mcu->updating) || READ_ONCE(mcu->in_boot))
		return -EBUSY;
	return __ohc_request(mcu, opcode, payload, len, out, OHC_TIMEOUT_MS);
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

	ret = ohc_request(mcu, OHC_OP_RELAY_GET, &sel, 1, &f);
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

	ret = ohc_request(mcu, OHC_OP_RELAY_TOGGLE, &sel, 1, &f);
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

	ret = ohc_request(mcu, OHC_OP_CONTACT_GET, NULL, 0, &f);
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
	struct ohc_frame f;
	unsigned int i, n;
	u32 carrier, word;
	u8 *payload;
	int ret;

	if (em->uart && READ_ONCE(em->uart->serial))
		return -EBUSY;		/* the jack is in serial mode */

	carrier = em->carrier ? em->carrier : 38000;
	word = OHC_IR_PRONTO_HZ / carrier;
	if (!word)
		return -EINVAL;		/* would be a divide-by-zero in the firmware */

	/* Truncating would transmit an unrecognisable code and report success. */
	if (count > IR_MAX_DURATIONS)
		return -EMSGSIZE;
	n = count;
	payload = kzalloc(OHC_IR_FIXED + n * 2, GFP_KERNEL);
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
		payload[OHC_IR_FIXED + i * 2] = periods >> 8;
		payload[OHC_IR_FIXED + i * 2 + 1] = periods & 0xff;
	}

	ret = ohc_request(mcu, OHC_OP_IROUT_SEND, payload, OHC_IR_FIXED + n * 2, &f);
	kfree(payload);
	if (ret)
		return ret;
	/* stock answers 0; openHC's says when it is busy or has no such output */
	if (f.len >= 1 && f.payload[0] == OHC_IR_ST_BUSY)
		return -EBUSY;
	if (f.len >= 1 && f.payload[0] != OHC_IR_ST_OK)
		return -EIO;
	return n;
}

static int ohc_tx_carrier(struct rc_dev *rcdev, u32 carrier)
{
	struct ohc_emitter *em = rcdev->priv;

	/* The firmware divides by the derived word; a carrier this low would
	 * round it to zero and take the part down with it. */
	if (carrier < 1000 || OHC_IR_PRONTO_HZ / carrier == 0)
		return -EINVAL;
	em->carrier = carrier;
	return 0;
}

/* The end of a code whose last chunk was full: no shorter one came after it. */
static void ohc_ir_idle(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, rx_idle);
	struct ir_raw_event ev = { .timeout = 1 };

	ir_raw_event_store(mcu->rx, &ev);
	ir_raw_event_handle(mcu->rx);
}

/*
 * A capture from the front receiver: [carrier period in 50 MHz ticks]
 * [bursts, bit 15 = mark, in carrier periods]. A long code is chunked at
 * OHC_IRIN_WORDS, each chunk repeating the period, so only a short chunk
 * ends it; after a full one the end is a quiet line.
 */
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
	carrier = OHC_IRIN_TICK_HZ / period;
	if (!carrier)
		return;
	cancel_delayed_work(&mcu->rx_idle);

	/* reaches lirc as LIRC_MODE2_FREQUENCY; the durations are microseconds */
	ev.carrier_report = 1;
	ev.carrier = carrier;
	ir_raw_event_store(mcu->rx, &ev);

	for (i = 2; i + 1 < len; i += 2) {
		u16 w = ((u16)p[i] << 8) | p[i + 1];

		memset(&ev, 0, sizeof(ev));
		if (!(w & 0x7fff))
			break;			/* padding, not a zero-length burst */
		ev.pulse = !!(w & OHC_IRIN_MARK);
		ev.duration = div_u64((u64)(w & 0x7fff) * 1000000u, carrier);
		ir_raw_event_store(mcu->rx, &ev);
	}
	if ((len - 2) / 2 >= OHC_IRIN_WORDS) {
		ir_raw_event_handle(mcu->rx);
		schedule_delayed_work(&mcu->rx_idle, msecs_to_jiffies(60));
		return;
	}
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
	return off < (unsigned int)mcu->geo.relays;
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
	return !!(mcu->contact_mask & BIT(off - mcu->geo.relays));
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

	/* A failed poll drops the cache: a contact read from a part that has
	 * stopped answering must fail, not repeat what it last said. */
	if (!ohc_contacts_get(mcu, &mask)) {
		mcu->contact_mask = mask;
		mcu->contacts_valid = true;
	} else {
		mcu->contacts_valid = false;
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
	if (opcode == OHC_OP_IRIN_CAPTURED)
		ohc_ir_capture(mcu, mcu->body + 5, len);
	else if (opcode == OHC_OP_UART_RECEIVE)
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

	if (idx < mcu->geo.relays) {
		ret = ohc_relay_get(mcu, idx, &on);
		if (ret)
			return ret;
	} else {
		u32 mask;

		ret = ohc_contacts_get(mcu, &mask);
		if (ret)
			return ret;
		on = !!(mask & BIT(idx - mcu->geo.relays));
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

	if (idx >= mcu->geo.relays)
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
 * multiplexed onto the same UART as the relays and the IR — the n_gsm pattern,
 * a serial device that offers ttys of its own.
 *
 * The port is switched to serial (UART_SET_CONTROL) on OPEN, not at probe, so
 * the IR emitter that shares the connector keeps working until a console is
 * actually asked for. Every UART opcode is unacknowledged, so TX goes through
 * ohc_send rather than ohc_request.
 */

/* Push the current line settings to the part. Fire-and-forget. */
static int ohc_uart_control(struct ohc_uart *u, bool enable)
{
	u8 pl[6];

	pl[0] = (u8)u->index;
	pl[1] = enable;			/* 1 serial, 0 back to IR */
	pl[2] = ohc_uart_baud_index(u->baud);
	pl[3] = u->data_bits;
	pl[4] = u->parity;
	pl[5] = u->stop_bits;
	return ohc_send(u->mcu, OHC_OP_UART_SET_CONTROL, pl, sizeof(pl));
}

static int ohc_uart_apply(struct ohc_uart *u)
{
	return ohc_uart_control(u, true);
}

static int ohc_serial_activate(struct tty_port *port, struct tty_struct *tty)
{
	struct ohc_uart *u = container_of(port, struct ohc_uart, port);

	/* A combo jack opens as serial only once its mode says so. */
	if (!READ_ONCE(u->serial))
		return -EBUSY;
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
		if (ohc_send(u->mcu, OHC_OP_UART_SEND, pl, n + 1))
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
	if (READ_ONCE(u->serial))
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

/*
 * /sys/class/tty/ttyOHC<n>/{jack,label,mode}. mode is "ir" or "serial"; a
 * combo jack starts as IR, and the tty opens only in serial mode.
 */
static ssize_t jack_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_uart *u = dev_get_drvdata(d);

	return sysfs_emit(buf, "%u\n", u->jack);
}
static DEVICE_ATTR_RO(jack);

static ssize_t label_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_uart *u = dev_get_drvdata(d);

	return sysfs_emit(buf, "%s\n", u->label ? u->label : "");
}
static DEVICE_ATTR_RO(label);

static ssize_t mode_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_uart *u = dev_get_drvdata(d);

	return sysfs_emit(buf, "%s\n", READ_ONCE(u->serial) ? "serial" : "ir");
}

static ssize_t mode_store(struct device *d, struct device_attribute *a,
			  const char *buf, size_t len)
{
	struct ohc_uart *u = dev_get_drvdata(d);
	bool serial;
	int ret = len;

	if (sysfs_streq(buf, "serial"))
		serial = true;
	else if (sysfs_streq(buf, "ir"))
		serial = false;
	else
		return -EINVAL;
	if (!serial && !u->ir)
		return -EINVAL;		/* no IR output on this connector */

	mutex_lock(&u->port.mutex);
	if (serial == READ_ONCE(u->serial))
		goto out;
	if (!serial && tty_port_initialized(&u->port)) {
		ret = -EBUSY;		/* close the tty first */
		goto out;
	}
	WRITE_ONCE(u->serial, serial);
	if (!serial)
		ohc_uart_control(u, false);
out:
	mutex_unlock(&u->port.mutex);
	return ret;
}
static DEVICE_ATTR_RW(mode);

static struct attribute *ohc_uart_attrs[] = {
	&dev_attr_jack.attr,
	&dev_attr_label.attr,
	&dev_attr_mode.attr,
	NULL,
};
ATTRIBUTE_GROUPS(ohc_uart);

static const struct ohc_jack *ohc_jack_of_uart(struct ohc_iomcu *mcu, int uart)
{
	int i;

	for (i = 0; i < mcu->geo.n_jacks; i++)
		if (mcu->geo.jacks[i].uart == uart)
			return &mcu->geo.jacks[i];
	return NULL;
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
		const struct ohc_jack *j = ohc_jack_of_uart(mcu, i);

		u->jack = j ? j->num : 0;
		u->label = j ? j->label : NULL;
		u->serial = !(j && j->ir >= 0);
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
		tty_port_register_device_attr(&mcu->uarts[i].port, drv, i, mcu->dev,
					      &mcu->uarts[i], ohc_uart_groups);

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
 * Reasoning at OHC_IR_PRONTO_HZ.
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
	int i, k = 0, ok = 0;

	if (n > 0) {
		mcu->emitters = kcalloc(n, sizeof(*mcu->emitters), GFP_KERNEL);
		if (!mcu->emitters)
			return;
		mcu->n_emitters = n;

		/*
		 * Named by panel jack number; the front blaster is not a jack and
		 * takes its own channel, or the one after the last jack.
		 */
		for (i = 0; i < mcu->geo.n_jacks; i++) {
			const struct ohc_jack *j = &mcu->geo.jacks[i];
			struct ohc_emitter *em = &mcu->emitters[k++];
			int ret;

			if (j->ir < 0) {
				k--;
				continue;
			}
			ret = ohc_register_emitter(mcu, em, 1u << j->ir,
						   kasprintf(GFP_KERNEL, "openHC IR out %u",
							     j->num));
			if (ret) {
				pr_warn("%s: IR out %u did not register: %d\n",
					mcu->name, j->num, ret);
				continue;
			}
			ok++;
			if (j->uart >= 0 && j->uart < mcu->n_uarts) {
				em->uart = &mcu->uarts[j->uart];
				mcu->uarts[j->uart].ir = em;
			}
		}
		if (mcu->geo.ir_blaster) {
			int ret = ohc_register_emitter(mcu, &mcu->emitters[k],
						       1u << ohc_blaster_bit(&mcu->geo),
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
				ohc_send(mcu, OHC_OP_IRIN_SET_CAPTURE, &on, 1);
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

static void ohc_pulse_reset(struct ohc_iomcu *mcu)
{
	gpiod_set_value_cansleep(mcu->reset, 1);
	msleep(20);
	gpiod_set_value_cansleep(mcu->reset, 0);
	msleep(200);
}

static const u8 ohc_boot_ack[] = { 0x00, OHC_BL_ACK };	/* TI bootloader ACK */
static const u8 ohc_app_frame[] = { DLE, STX };		/* anything the app says */
static const u8 ohc_knock[] = { OHC_KNOCK, OHC_KNOCK };
static const u8 ohc_ping[] = { 0x03, OHC_BL_PING, OHC_BL_PING };

static bool ohc_raw_answered(struct ohc_iomcu *mcu)
{
	return ohc_raw_seen(mcu, ohc_boot_ack, sizeof(ohc_boot_ack)) ||
	       ohc_raw_seen(mcu, ohc_app_frame, sizeof(ohc_app_frame));
}

/*
 * Send @out on a clean slate, then wait up to @ms for @needle to show up in
 * what comes back. No needle: wait for either the bootloader's ACK or an
 * application frame, whichever comes; the caller looks at which.
 */
static bool ohc_raw_xfer(struct ohc_iomcu *mcu, const u8 *out, int len,
			 const u8 *needle, int n, int ms)
{
	ohc_raw_reset(mcu);
	if (ohc_xmit(mcu, out, len))
		return false;
	if (!needle)
		return wait_event_timeout(mcu->raw_wait, ohc_raw_answered(mcu),
					  msecs_to_jiffies(ms)) > 0;
	return wait_event_timeout(mcu->raw_wait, ohc_raw_seen(mcu, needle, n),
				  msecs_to_jiffies(ms)) > 0;
}

/* Raw mode: the link is ours, bytes collect in raw_buf, no frame goes out. */
static void ohc_raw_begin(struct ohc_iomcu *mcu)
{
	mutex_lock(&mcu->io_lock);
	WRITE_ONCE(mcu->raw, true);
	ohc_raw_reset(mcu);
}

static void ohc_raw_end(struct ohc_iomcu *mcu)
{
	/* Whatever the decoder held from before is stale; hunt afresh. */
	mcu->state = RX_IDLE;
	mcu->body_len = 0;
	WRITE_ONCE(mcu->raw, false);
	mutex_unlock(&mcu->io_lock);
}

/*
 * COMMAND_RUN at the app slot, then knock for the application. True if it
 * answered. If the bootloader answers instead, nothing started (an empty or
 * half-written slot, whose stub resets straight back) and in_boot says so.
 * Raw mode.
 */
static bool ohc_run_app(struct ohc_iomcu *mcu)
{
	/* [size][sum][RUN, address BE]: never ACKed, it has jumped by then */
	static const u8 run[] = { 0x07, OHC_BL_RUN + 0x10, OHC_BL_RUN, 0x00, 0x00, 0x10, 0x00 };

	ohc_raw_reset(mcu);
	ohc_xmit(mcu, run, sizeof(run));
	msleep(300);
	ohc_raw_xfer(mcu, ohc_knock, sizeof(ohc_knock), NULL, 0, 700);
	if (ohc_raw_seen(mcu, ohc_app_frame, sizeof(ohc_app_frame))) {
		WRITE_ONCE(mcu->in_boot, false);
		return true;
	}
	WRITE_ONCE(mcu->in_boot, ohc_raw_seen(mcu, ohc_boot_ack, sizeof(ohc_boot_ack)));
	return false;
}

/*
 * Start the application. The part (TM4C on EA, LM3S on HC) powers up in the TI
 * serial bootloader and does NOT run its application on its own:
 *
 *   autobaud      55 55                  -> 00 cc  (bootloader ACK)
 *   PING          03 20 20               -> 00 cc
 *   COMMAND_RUN   07 32 22 00 00 10 00   -> nothing: it has jumped to 0x1000
 *   app autobaud  55 55                  -> 10 02 d7 ... (AUTO_BAUD_RESP)
 *
 * THE KNOCK GOES FIRST, before any DLE/STX: the bootloader autobauds on the
 * first bytes it hears, and a frame before the knock leaves it deaf. A running
 * application answers the same knock with AUTO_BAUD_RESP, so one knock tells
 * the two apart and a part already up (a warm reboot) is left alone. A part
 * that answers nothing gets one pulse of reset-gpios and a second knock: an HC-250
 * kexec'd from stock finds the app mute (stock ran it at 460800).
 * It is also why a reset is recovered by rebinding, never in place: iod's MCU
 * reset unbinds, pulses, and binds again, so this runs first.
 */
static void ohc_bringup(struct ohc_iomcu *mcu)
{
	bool answered, boot = false, up = false;

	ohc_raw_begin(mcu);
	WRITE_ONCE(mcu->in_boot, false);
	answered = ohc_raw_xfer(mcu, ohc_knock, sizeof(ohc_knock), NULL, 0, 400);
	if (!answered && mcu->reset) {
		ohc_pulse_reset(mcu);
		answered = ohc_raw_xfer(mcu, ohc_knock, sizeof(ohc_knock), NULL, 0, 400);
		if (answered)
			pr_info("%s: silent until reset\n", mcu->name);
	}
	if (answered)
		boot = ohc_raw_seen(mcu, ohc_boot_ack, sizeof(ohc_boot_ack));
	if (boot) {
		/* PING mostly confirms the link before the un-ACKed RUN. */
		ohc_raw_xfer(mcu, ohc_ping, sizeof(ohc_ping), ohc_boot_ack,
			     sizeof(ohc_boot_ack), 400);
		up = ohc_run_app(mcu);
	}
	ohc_raw_end(mcu);

	/* Silence is not reported here: FIRMWARE_VERSION is the verdict. */
	if (up)
		pr_info("%s: started the application from the TI bootloader\n", mcu->name);
	else if (READ_ONCE(mcu->in_boot))
		pr_warn("%s: the bootloader answers but its application did not start\n",
			mcu->name);
}

static bool ohc_ours(struct ohc_iomcu *mcu)
{
	return str_has_prefix(mcu->fw_version, OHC_FW_VERSION_PREFIX);
}

/* FIRMWARE_VERSION: online, and what runs. */
static int ohc_read_version(struct ohc_iomcu *mcu)
{
	struct ohc_frame f;
	int n, ret;

	ret = __ohc_request(mcu, OHC_OP_FIRMWARE_VERSION, NULL, 0, &f, OHC_TIMEOUT_MS);
	if (ret) {
		WRITE_ONCE(mcu->online, false);
		return ret;
	}
	n = min_t(int, f.len, sizeof(mcu->fw_version) - 1);
	memcpy(mcu->fw_version, f.payload, n);
	mcu->fw_version[n] = '\0';
	WRITE_ONCE(mcu->online, true);
	return 0;
}

/* ---- the pin table ------------------------------------------------------ */

static const char *const ohc_cfg_kind[] = {
	[OHC_CFG_IR_OUT] = "IR output", [OHC_CFG_IR_IN] = "IR receiver",
	[OHC_CFG_RELAY] = "relay", [OHC_CFG_CONTACT] = "contact",
	[OHC_CFG_UART_RX] = "serial RX", [OHC_CFG_UART_TX] = "serial TX",
	[OHC_CFG_IR_CARRIER] = "IR carrier", [OHC_CFG_IR_GATE] = "IR gate",
	[OHC_CFG_HOLD] = "held line", [OHC_CFG_HEARTBEAT] = "heartbeat",
	[OHC_CFG_UART_MUX_A] = "serial mode line A", [OHC_CFG_UART_MUX_B] = "serial mode line B",
};

static const char *const ohc_cfg_status[] = {
	[OHC_CFG_S_KIND] = "unknown kind", [OHC_CFG_S_INDEX] = "index out of range or used twice",
	[OHC_CFG_S_PIN] = "no such pin", [OHC_CFG_S_RESERVED] = "pin reserved (host link or debug)",
	[OHC_CFG_S_FUNC] = "the pin cannot do this", [OHC_CFG_S_BUSY] = "pin or peripheral already taken",
	[OHC_CFG_S_PAIR] = "its partner is missing (UART RX/TX, a gate's carrier)",
	[OHC_CFG_S_FLAGS] = "bad flags",
};

static void ohc_cfg_add(u8 *p, int *n, u8 kind, int index, struct ohc_pin pin, u8 flags)
{
	u8 *e = p + OHC_CFG_HDR + *n * OHC_CFG_ENTRY;

	if (!pin.set || index < 0 || *n >= OHC_CFG_MAX_ENTRIES)
		return;
	e[0] = kind;
	e[1] = index;
	e[2] = pin.port;
	e[3] = pin.pin;
	e[4] = flags;
	(*n)++;
}

/*
 * CONFIG_SET: every pin the node describes, as {kind, index, port, pin,
 * flags}. openHC's firmware starts with nothing driven and applies this,
 * NAKing per entry what the part cannot do. Control4's firmware is not sent it.
 */
static int ohc_send_config(struct ohc_iomcu *mcu)
{
	const struct ohc_geometry *geo = &mcu->geo;
	struct ohc_frame *f;
	int i, n = 0, bad = 0, ret;
	u8 *p, gate_flags;

	if (!ohc_ours(mcu))
		return 0;

	p = kzalloc(OHC_CFG_HDR + OHC_CFG_MAX_ENTRIES * OHC_CFG_ENTRY, GFP_KERNEL);
	f = kmalloc(sizeof(*f), GFP_KERNEL);
	if (!p || !f) {
		ret = -ENOMEM;
		goto out;
	}
	gate_flags = geo->gates_active_low ? OHC_CFG_F_ACTIVE_LOW : 0;
	for (i = 0; i < OHC_CFG_CARRIERS; i++)
		ohc_cfg_add(p, &n, OHC_CFG_IR_CARRIER, i, geo->carrier[i], 0);
	for (i = 0; i < geo->n_jacks; i++) {
		const struct ohc_jack *j = &geo->jacks[i];
		int k;

		ohc_cfg_add(p, &n, OHC_CFG_IR_OUT, j->ir, j->ir_pin, 0);
		for (k = 0; k < j->n_gates; k++)
			ohc_cfg_add(p, &n, OHC_CFG_IR_GATE, j->ir, j->gate[k].pin,
				    gate_flags | OHC_CFG_F_CARRIER(j->gate[k].carrier));
		if (j->rx.set && j->tx.set) {
			ohc_cfg_add(p, &n, OHC_CFG_UART_RX, j->uart, j->rx, 0);
			ohc_cfg_add(p, &n, OHC_CFG_UART_TX, j->uart, j->tx, 0);
		}
		ohc_cfg_add(p, &n, OHC_CFG_UART_MUX_A, j->uart, j->mux_a, 0);
		ohc_cfg_add(p, &n, OHC_CFG_UART_MUX_B, j->uart, j->mux_b, 0);
	}
	if (geo->ir_blaster) {
		int bit = ohc_blaster_bit(geo);

		ohc_cfg_add(p, &n, OHC_CFG_IR_OUT, bit, geo->blaster_pin, 0);
		for (i = 0; i < geo->n_blaster_gates; i++)
			ohc_cfg_add(p, &n, OHC_CFG_IR_GATE, bit, geo->blaster_gate[i].pin,
				    gate_flags | OHC_CFG_F_CARRIER(geo->blaster_gate[i].carrier));
	}
	for (i = 0; i < geo->n_hold; i++)
		ohc_cfg_add(p, &n, OHC_CFG_HOLD, i, geo->hold[i], 0);
	ohc_cfg_add(p, &n, OHC_CFG_HEARTBEAT, 0, geo->heartbeat, 0);
	if (geo->ir_in)
		ohc_cfg_add(p, &n, OHC_CFG_IR_IN, 0, geo->rx_pin, geo->rx_flags);
	for (i = 0; i < geo->relays; i++)
		ohc_cfg_add(p, &n, OHC_CFG_RELAY, i, geo->relay[i].pin, geo->relay[i].flags);
	for (i = 0; i < geo->contacts; i++)
		ohc_cfg_add(p, &n, OHC_CFG_CONTACT, i, geo->contact[i].pin, geo->contact[i].flags);

	ret = 0;
	if (!n) {
		dev_warn(mcu->dev, "the node gives no pins: openHC's firmware drives nothing\n");
		goto out;
	}
	p[0] = OHC_CFG_VERSION;
	p[1] = n;
	ret = __ohc_request(mcu, OHC_OP_CONFIG_SET, p, OHC_CFG_HDR + n * OHC_CFG_ENTRY, f,
			    OHC_TIMEOUT_MS);
	if (ret) {
		dev_err(mcu->dev, "no answer to the pin table: %d\n", ret);
		goto out;
	}
	if (f->opcode != OHC_OP_CONFIG_RESP || f->len < 3 || f->payload[0] != OHC_CFG_VERSION) {
		dev_err(mcu->dev, "malformed answer to the pin table\n");
		ret = -EPROTO;
		goto out;
	}
	if (f->payload[1] != OHC_CFG_R_OK) {
		dev_err(mcu->dev, "the firmware refused the pin table (%u); it drives nothing\n",
			f->payload[1]);
		ret = -EINVAL;
		goto out;
	}
	for (i = 0; i < n && i + 3 < f->len; i++) {
		const u8 *e = p + OHC_CFG_HDR + i * OHC_CFG_ENTRY;
		u8 st = f->payload[3 + i];

		if (st == OHC_CFG_S_OK)
			continue;
		bad++;
		dev_err(mcu->dev, "%s %u on P%c%u: %s\n",
			e[0] < ARRAY_SIZE(ohc_cfg_kind) ? ohc_cfg_kind[e[0]] : "?", e[1],
			'A' + e[2], e[3],
			st < ARRAY_SIZE(ohc_cfg_status) && ohc_cfg_status[st] ? ohc_cfg_status[st] : "refused");
	}
	dev_info(mcu->dev, "pin table: %d of %d entries applied\n", n - bad, n);
out:
	kfree(f);
	kfree(p);
	return ret;
}

/* ---- registration ------------------------------------------------------- */

/* The chip, the lirc devices and the ttys, once the part has answered. */
static void ohc_register_all(struct ohc_iomcu *mcu)
{
	int i, lines;

	if (mcu->registered)
		return;
	mcu->registered = true;

	lines = mcu->geo.relays + mcu->geo.contacts;
	if (!lines)
		goto rest;	/* no GPIO lines (the EA1): IR and serial only */
	mcu->names = kcalloc(lines, sizeof(char *), GFP_KERNEL);
	if (!mcu->names)
		goto rest;
	/*
	 * Named lines, so `gpiofind relay1` works. ONE-BASED, because that is
	 * what is printed on the back of the box; the line offset and the wire
	 * selector stay zero-based, as they must.
	 */
	for (i = 0; i < lines; i++) {
		mcu->names[i] = kasprintf(GFP_KERNEL, "%s%d",
					  i < mcu->geo.relays ? "relay" : "contact",
					  (i < mcu->geo.relays ? i : i - mcu->geo.relays) + 1);
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
		mcu->name, mcu->geo.relays, mcu->geo.contacts);
	goto rest;

err_names:
	for (i = 0; i < lines; i++)
		kfree(mcu->names[i]);
	kfree(mcu->names);
	mcu->names = NULL;
rest:
	ohc_register_serial(mcu);
	ohc_register_rc(mcu);
}

/*
 * Start the application if it needs starting, identify it, send the pin
 * table, then register. Deferred from probe: the bring-up can take a second.
 * A part that never answers gets no chip — a gpiochip standing in for a silent
 * part is one whose every read is a lie — unless the loader brings it up.
 */
static void ohc_identify(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, identify_work);

	ohc_bringup(mcu);
	if (!READ_ONCE(mcu->in_boot) && !ohc_read_version(mcu)) {
		dev_info(mcu->dev, "firmware %s\n", mcu->fw_version);
		ohc_send_config(mcu);
		ohc_register_all(mcu);
	} else if (!mcu->fw_name) {
		pr_warn("no reply from the IO microcontroller on %s; no gpiochip registered\n",
			mcu->name);
		pr_warn("  tx=%lu rx=%lu frames=%lu badcsum=%lu\n",
			mcu->tx_bytes, mcu->rx_bytes, mcu->rx_frames, mcu->rx_bad_csum);
		if (mcu->first_rx_len)
			print_hex_dump(KERN_WARNING, "ohc-iomcu: first rx: ",
				       DUMP_PREFIX_NONE, 16, 1,
				       mcu->first_rx, mcu->first_rx_len, false);
	}
	if (mcu->fw_name)
		schedule_delayed_work(&mcu->fw_work, 0);
}

/* ---- the loader: TI serial bootloader ----------------------------------- */

/* The next byte from the part, in raw mode. */
static int ohc_raw_byte(struct ohc_iomcu *mcu, int *pos, int ms)
{
	if (!wait_event_timeout(mcu->raw_wait, READ_ONCE(mcu->raw_len) > *pos,
				msecs_to_jiffies(ms)))
		return -ETIMEDOUT;
	return mcu->raw_buf[(*pos)++];
}

/* The bootloader's answer to a packet: zeros, then ACK or NAK. */
static int ohc_bl_ack(struct ohc_iomcu *mcu, int *pos, int ms)
{
	int c;

	do {
		c = ohc_raw_byte(mcu, pos, ms);
	} while (c == 0);
	if (c < 0)
		return c;
	return c == OHC_BL_ACK ? 0 : c == OHC_BL_NAK ? -EIO : -EPROTO;
}

/* [size][sum][data], and its ACK. */
static int ohc_bl_packet(struct ohc_iomcu *mcu, const u8 *d, int n, int ms)
{
	u8 buf[2 + 1 + OHC_BL_CHUNK];
	int i, pos = 0, ret;

	buf[0] = n + 2;
	buf[1] = 0;
	for (i = 0; i < n; i++)
		buf[1] += d[i];
	memcpy(buf + 2, d, n);
	ohc_raw_reset(mcu);
	ret = ohc_xmit(mcu, buf, n + 2);
	return ret ? ret : ohc_bl_ack(mcu, &pos, ms);
}

/* GET_STATUS: its answer is a packet of its own, which must be ACKed. */
static int ohc_bl_status(struct ohc_iomcu *mcu)
{
	static const u8 get[] = { 0x03, OHC_BL_GET_STATUS, OHC_BL_GET_STATUS };
	static const u8 ack[] = { OHC_BL_ACK };
	int pos = 0, size, sum, st, ret;

	ohc_raw_reset(mcu);
	ret = ohc_xmit(mcu, get, sizeof(get));
	if (!ret)
		ret = ohc_bl_ack(mcu, &pos, 500);
	if (ret)
		return ret;
	do {
		size = ohc_raw_byte(mcu, &pos, 500);
	} while (size == 0);
	sum = ohc_raw_byte(mcu, &pos, 500);
	st = ohc_raw_byte(mcu, &pos, 500);
	if (size < 0 || sum < 0 || st < 0)
		return -ETIMEDOUT;
	ohc_xmit(mcu, ack, sizeof(ack));
	return size == 3 && sum == st ? st : -EPROTO;
}

static int ohc_bl_cmd(struct ohc_iomcu *mcu, const u8 *d, int n, int ms)
{
	int st = ohc_bl_packet(mcu, d, n, ms);

	if (!st)
		st = ohc_bl_status(mcu);
	return st < 0 ? st : st == OHC_BL_SUCCESS ? 0 : -EIO;
}

/* DOWNLOAD erases [addr, addr + size); SEND_DATA then programs it in order. */
static int ohc_bl_download(struct ohc_iomcu *mcu, u32 addr, const u8 *p, u32 size)
{
	u8 c[1 + OHC_BL_CHUNK];
	u32 off, n;
	int ret;

	c[0] = OHC_BL_DOWNLOAD;
	put_unaligned_be32(addr, c + 1);
	put_unaligned_be32(size, c + 5);
	ret = ohc_bl_cmd(mcu, c, 9, 5000);	/* the erase happens before the ACK */
	for (off = 0; !ret && off < size; off += n) {
		if (READ_ONCE(mcu->fw_cancel))
			return -ECANCELED;
		n = min_t(u32, size - off, OHC_BL_CHUNK);
		c[0] = OHC_BL_SEND_DATA;
		memcpy(c + 1, p + off, n);
		ret = ohc_bl_cmd(mcu, c, n + 1, 500);
	}
	return ret;
}

/*
 * A vector page that resets the part: SP, reset -> ldr r0/r1, str, b . with
 * AIRCR <- VECTKEY | SYSRESETREQ. While the slot holds it, starting the
 * application lands back in the bootloader.
 */
static const u8 ohc_reset_stub[] = {
	0x00, 0x04, 0x00, 0x20,	0x09, 0x10, 0x00, 0x00,
	0x01, 0x48, 0x02, 0x49,	0x01, 0x60, 0xfe, 0xe7,
	0x0c, 0xed, 0x00, 0xe0,	0x04, 0x00, 0xfa, 0x05,
};

/*
 * The application slot, its first page last. The bootloader takes DOWNLOAD only
 * at the slot's base (and erases from there), so: the whole slot once with the
 * stub in place of the first page, then the first page alone. Cut short
 * anywhere before that last page, the slot starts into the stub, which resets
 * into the bootloader, and the next probe finds the part there and writes it
 * again. The bootloader's 4 KB below OHC_APP_BASE is never addressed.
 */
static int ohc_bl_write(struct ohc_iomcu *mcu, const struct ohc_img *img)
{
	u32 first = min_t(u32, img->size, OHC_FLASH_PAGE);
	u8 *slot;
	int ret;

	if (img->size <= first)
		return ohc_bl_download(mcu, OHC_APP_BASE, img->data, img->size);
	slot = kmemdup(img->data, img->size, GFP_KERNEL);
	if (!slot)
		return -ENOMEM;
	memset(slot, 0xff, first);
	memcpy(slot, ohc_reset_stub, sizeof(ohc_reset_stub));
	ret = ohc_bl_download(mcu, OHC_APP_BASE, slot, img->size);
	kfree(slot);
	if (!ret)
		ret = ohc_bl_download(mcu, OHC_APP_BASE, img->data, first);
	return ret;
}

/*
 * Into the bootloader, synced: a bootloader already there answers PING; ours
 * resets on OHC_OP_RESET; anything else needs the reset line. Raw mode, so no
 * frame reaches a fresh bootloader before its knock.
 */
static int ohc_enter_boot(struct ohc_iomcu *mcu)
{
	if (READ_ONCE(mcu->in_boot) &&
	    ohc_raw_xfer(mcu, ohc_ping, sizeof(ohc_ping), ohc_boot_ack, sizeof(ohc_boot_ack), 400))
		return 0;
	if (READ_ONCE(mcu->online) && ohc_ours(mcu)) {
		ohc_write_frame(mcu, OHC_OP_RESET, ++mcu->seq, NULL, 0);
		msleep(200);
	} else if (mcu->reset) {
		ohc_pulse_reset(mcu);
	} else if (READ_ONCE(mcu->online)) {
		return -EOPNOTSUPP;	/* stock, and no way to reset it */
	}
	WRITE_ONCE(mcu->online, false);
	if (!ohc_raw_xfer(mcu, ohc_knock, sizeof(ohc_knock), ohc_boot_ack,
			  sizeof(ohc_boot_ack), 400))
		return -ENODEV;
	WRITE_ONCE(mcu->in_boot, true);
	return 0;
}

/* The header, checked against this part; img->data points into @d. */
static int ohc_img_parse(struct ohc_iomcu *mcu, const u8 *d, size_t n, struct ohc_img *img)
{
	if (n < OHC_IMG_HDR_LEN || memcmp(d, OHC_IMG_MAGIC, 8) ||
	    get_unaligned_le16(d + OHC_IMG_OFF_HDR_VER) != 1 ||
	    get_unaligned_le16(d + OHC_IMG_OFF_HDR_LEN) != OHC_IMG_HDR_LEN ||
	    ~crc32_le(~0, d, OHC_IMG_OFF_HDR_CRC) != get_unaligned_le32(d + OHC_IMG_OFF_HDR_CRC))
		return -EINVAL;
	img->part = get_unaligned_le32(d + OHC_IMG_OFF_PART);
	img->load = get_unaligned_le32(d + OHC_IMG_OFF_LOAD);
	img->size = get_unaligned_le32(d + OHC_IMG_OFF_SIZE);
	if (img->part != mcu->part)
		return -ENODEV;
	if (img->load != OHC_APP_BASE || !img->size || img->size % 4 ||
	    img->size > OHC_FLASH_SIZE - OHC_APP_BASE || img->size != n - OHC_IMG_HDR_LEN)
		return -EINVAL;
	if (~crc32_le(~0, d + OHC_IMG_HDR_LEN, img->size) != get_unaligned_le32(d + OHC_IMG_OFF_CRC))
		return -EBADMSG;
	memcpy(img->version, d + OHC_IMG_OFF_VERSION, OHC_IMG_VERSION_LEN);
	img->version[OHC_IMG_VERSION_LEN - 1] = '\0';
	img->data = d + OHC_IMG_HDR_LEN;
	return 0;
}

/*
 * Write @img, start it, check it answers with its own version, send the pin
 * table. Every other request is refused meanwhile. On failure the part is left
 * in its bootloader (in_boot), where the next probe finds it and writes again.
 */
static int ohc_flash(struct ohc_iomcu *mcu, const struct ohc_img *img)
{
	int ret = -ENODEV, tries;

	mutex_lock(&mcu->fw_lock);
	WRITE_ONCE(mcu->updating, true);
	mcu->contacts_valid = false;
	ohc_raw_begin(mcu);
	for (tries = 0; tries < 3; tries++) {
		ret = ohc_enter_boot(mcu);
		if (!ret)
			ret = ohc_bl_write(mcu, img);
		if (!ret || ret == -ECANCELED || ret == -EOPNOTSUPP)
			break;
		dev_warn(mcu->dev, "write %d of 3 failed: %d\n", tries + 1, ret);
	}
	if (!ret && !ohc_run_app(mcu))
		ret = -EIO;
	ohc_raw_end(mcu);
	WRITE_ONCE(mcu->updating, false);

	if (!ret)
		ret = ohc_read_version(mcu);
	if (!ret && strcmp(mcu->fw_version, img->version)) {
		dev_err(mcu->dev, "wrote %s, but the part says %s\n", img->version, mcu->fw_version);
		ret = -EIO;
	}
	if (!ret)
		ohc_send_config(mcu);
	mutex_unlock(&mcu->fw_lock);
	return ret;
}

/*
 * firmware-name: load it, and write it only when the part runs something
 * else. Built in, this can run before the root filesystem is mounted, so a
 * missing file is retried for a while.
 */
static void ohc_fw_work(struct work_struct *work)
{
	struct ohc_iomcu *mcu = container_of(to_delayed_work(work),
					     struct ohc_iomcu, fw_work);
	const struct firmware *fw;
	struct ohc_img img;
	int ret;

	ret = request_firmware_direct(&fw, mcu->fw_name, mcu->dev);
	if (ret == -ENOENT && ++mcu->fw_tries < 90) {
		schedule_delayed_work(&mcu->fw_work, 2 * HZ);
		return;
	}
	if (ret) {
		dev_warn(mcu->dev, "cannot load %s: %d\n", mcu->fw_name, ret);
		return;
	}
	ret = ohc_img_parse(mcu, fw->data, fw->size, &img);
	if (ret) {
		dev_err(mcu->dev, "%s is not an image for this part: %d\n", mcu->fw_name, ret);
		goto out;
	}
	if (READ_ONCE(mcu->online) && !strcmp(mcu->fw_version, img.version))
		goto out;
	if (READ_ONCE(mcu->online) && !ohc_ours(mcu) && !mcu->reset) {
		dev_warn(mcu->dev, "runs %s; replacing it needs reset-gpios\n", mcu->fw_version);
		goto out;
	}
	dev_info(mcu->dev, "writing %s (%u bytes) over %s\n", img.version, img.size,
		 READ_ONCE(mcu->online) ? mcu->fw_version : "no running application");
	ret = ohc_flash(mcu, &img);
	if (ret) {
		dev_err(mcu->dev, "writing %s failed: %d%s\n", img.version, ret,
			READ_ONCE(mcu->in_boot) ? "; left in its bootloader for the next probe" : "");
		goto out;
	}
	dev_info(mcu->dev, "firmware %s\n", mcu->fw_version);
	ohc_register_all(mcu);
out:
	release_firmware(fw);
}

/*
 * /sys/class/firmware/ohc-iomcu: any image for this part, written on request
 * whatever runs now. Return to stock puts Control4's back this way.
 */
static enum fw_upload_err ohc_up_prepare(struct fw_upload *fwl, const u8 *data, u32 size)
{
	struct ohc_iomcu *mcu = fwl->dd_handle;

	WRITE_ONCE(mcu->fw_cancel, false);
	if (ohc_img_parse(mcu, data, size, &mcu->upload))
		return FW_UPLOAD_ERR_FW_INVALID;
	return FW_UPLOAD_ERR_NONE;
}

static enum fw_upload_err ohc_up_write(struct fw_upload *fwl, const u8 *data,
				       u32 offset, u32 size, u32 *written)
{
	struct ohc_iomcu *mcu = fwl->dd_handle;
	int ret;

	*written = size;
	if (offset)
		return FW_UPLOAD_ERR_NONE;
	ret = ohc_flash(mcu, &mcu->upload);
	if (!ret) {
		dev_info(mcu->dev, "wrote firmware %s\n", mcu->fw_version);
		ohc_register_all(mcu);
	}
	switch (ret) {
	case 0:			return FW_UPLOAD_ERR_NONE;
	case -ECANCELED:	return FW_UPLOAD_ERR_CANCELED;
	case -ETIMEDOUT:	return FW_UPLOAD_ERR_TIMEOUT;
	case -EOPNOTSUPP:	return FW_UPLOAD_ERR_HW_ERROR;
	default:		return FW_UPLOAD_ERR_RW_ERROR;
	}
}

static enum fw_upload_err ohc_up_poll(struct fw_upload *fwl)
{
	return FW_UPLOAD_ERR_NONE;
}

static void ohc_up_cancel(struct fw_upload *fwl)
{
	struct ohc_iomcu *mcu = fwl->dd_handle;

	WRITE_ONCE(mcu->fw_cancel, true);
}

static const struct fw_upload_ops ohc_up_ops = {
	.prepare	= ohc_up_prepare,
	.write		= ohc_up_write,
	.poll_complete	= ohc_up_poll,
	.cancel		= ohc_up_cancel,
};

/* ---- /sys/class/ohc/iomcu ----------------------------------------------- */

/*
 * The geometry, from the kernel rather than a second copy in userspace. The
 * firmware node (the board device tree on EA, the ACPI table on the HC-800)
 * says what sits behind the part, the driver latched it at probe, and this is
 * where iod reads it back:
 *
 *	/sys/class/ohc/iomcu/{relays,contacts,ir_out,ir_blaster,ir_in,uarts}
 *	/sys/class/ohc/iomcu/{port,baud,online,firmware,part}
 *
 * It appears at probe, not when the part answers: a box whose microcontroller
 * is silent should still say it has four relays, so the diagnosis is "not
 * answering", not "this board has no IO". online says which.
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

/* What FIRMWARE_VERSION said; empty until it answers. */
static ssize_t firmware_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_iomcu *mcu = dev_get_drvdata(d);

	return sysfs_emit(buf, "%s\n", mcu->fw_version);
}
static DEVICE_ATTR_RO(firmware);

static ssize_t part_show(struct device *d, struct device_attribute *a, char *buf)
{
	struct ohc_iomcu *mcu = dev_get_drvdata(d);

	return sysfs_emit(buf, "%s\n", mcu->part == OHC_PART_TM4C1231D5 ? "tm4c1231d5" :
					mcu->part == OHC_PART_LM3S1162 ? "lm3s1162" : "");
}
static DEVICE_ATTR_RO(part);

/* The serdev device the part is: serial0-0. */
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
	&dev_attr_firmware.attr,
	&dev_attr_part.attr,
	NULL,
};
ATTRIBUTE_GROUPS(ohc_info);

/* ---- serdev driver ------------------------------------------------------ *
 *
 * The board's firmware describes the part as a child of the UART it is wired
 * to, with what sits behind it: the board device tree on EA, a PRP0001 device
 * with the same properties in _DSD on the HC-800 (board/hc800/acpi/), so the
 * match table and the fwnode reads below are the same code. The binding is
 * Documentation/devicetree/bindings/gpio/control4,io-mcu.yaml.
 *
 * A UART with a serdev child is no longer a tty: the port belongs to this
 * driver from boot, and nothing in userspace attaches anything.
 */

static size_t ohc_receive_buf(struct serdev_device *serdev, const u8 *buf,
			      size_t count)
{
	ohc_feed(serdev_device_get_drvdata(serdev), buf, count);
	return count;
}

static const struct serdev_device_ops ohc_serdev_ops = {
	.receive_buf	= ohc_receive_buf,
	.write_wakeup	= serdev_device_write_wakeup,
};

/* An absent count is zero: a board describes only what it has. */
static int ohc_prop_count(struct device *dev, const char *prop)
{
	u32 v = 0;

	device_property_read_u32(dev, prop, &v);
	return v > 32 ? -1 : (int)v;
}

/* <port pin>, both in range; absent is fine, malformed is not. */
static int ohc_read_pins(struct fwnode_handle *node, const char *prop,
			 struct ohc_pin *pins, int n)
{
	u32 v[4];
	int i, ret;

	if (!fwnode_property_present(node, prop))
		return 0;
	ret = fwnode_property_read_u32_array(node, prop, v, 2 * n);
	if (ret)
		return ret;
	for (i = 0; i < n; i++) {
		if (v[2 * i] > 7 || v[2 * i + 1] > 7)
			return -EINVAL;
		pins[i] = (struct ohc_pin){ .set = true, .port = v[2 * i], .pin = v[2 * i + 1] };
	}
	return 0;
}

/* A list of <port pin> or <port pin carrier>: how many entries were read. */
static int ohc_read_list(struct fwnode_handle *node, const char *prop, int cells,
			 u32 *v, int max)
{
	int n, i;

	if (!fwnode_property_present(node, prop))
		return 0;
	n = fwnode_property_count_u32(node, prop);
	if (n <= 0 || n % cells || n / cells > max ||
	    fwnode_property_read_u32_array(node, prop, v, n))
		return -EINVAL;
	for (i = 0; i < n; i++)
		if (v[i] > ((i % cells) == 2 ? OHC_CFG_CARRIERS - 1 : 7))
			return -EINVAL;
	return n / cells;
}

static int ohc_read_gates(struct fwnode_handle *node, struct ohc_gate *g)
{
	u32 v[3 * OHC_MAX_GATES];
	int i, n = ohc_read_list(node, "control4,ir-gates", 3, v, OHC_MAX_GATES);

	for (i = 0; i < n; i++)
		g[i] = (struct ohc_gate){
			.pin = { .set = true, .port = v[3 * i], .pin = v[3 * i + 1] },
			.carrier = v[3 * i + 2],
		};
	return n;
}

/* control4,uart-mux-pins: line A, and line B where the jack has one (EA). */
static int ohc_read_mux(struct fwnode_handle *node, struct ohc_pin *mux)
{
	const char *prop = "control4,uart-mux-pins";
	int n;

	if (!fwnode_property_present(node, prop))
		return 0;
	n = fwnode_property_count_u32(node, prop);
	if (n != 2 && n != 4)
		return -EINVAL;
	return ohc_read_pins(node, prop, mux, n / 2);
}

static int ohc_parse_jack(struct device *dev, struct fwnode_handle *child,
			  struct ohc_jack *j)
{
	struct ohc_pin uart[2] = {}, mux[2] = {};
	u32 v;

	if (fwnode_property_read_u32(child, "reg", &j->num))
		return -EINVAL;
	fwnode_property_read_string(child, "label", &j->label);
	j->ir = fwnode_property_read_u32(child, "control4,ir-channel", &v) ? -1 : v;
	j->uart = fwnode_property_read_u32(child, "control4,uart", &v) ? -1 : v;
	if (j->ir >= OHC_IR_MAX_OUTPUTS || j->uart > 31 || (j->ir < 0 && j->uart < 0))
		return dev_err_probe(dev, -EINVAL, "jack %u: bad channel or uart\n", j->num);
	j->n_gates = ohc_read_gates(child, j->gate);
	if (j->n_gates < 0 || ohc_read_pins(child, "control4,ir-pin", &j->ir_pin, 1) ||
	    ohc_read_pins(child, "control4,uart-pins", uart, 2) ||
	    ohc_read_mux(child, mux))
		return dev_err_probe(dev, -EINVAL, "jack %u: bad pin\n", j->num);
	j->rx = uart[0];
	j->tx = uart[1];
	j->mux_a = mux[0];
	j->mux_b = mux[1];
	return 0;
}

/*
 * relays { relay@N } or contacts { contact@N }: N is the wire index and the
 * line, numbered 0.. with no gaps; control4,pin where it is known. Without
 * the group, the count property says how many there are.
 */
static int ohc_parse_lines(struct device *dev, const char *group, const char *count,
			   int *n, struct ohc_line **out)
{
	struct fwnode_handle *g, *child;
	struct ohc_line *l;
	u32 reg;

	*n = ohc_prop_count(dev, count);
	if (*n < 0)
		return dev_err_probe(dev, -EINVAL, "%s: implausible\n", count);
	g = device_get_named_child_node(dev, group);
	if (!g) {
		*out = devm_kcalloc(dev, max(*n, 1), sizeof(**out), GFP_KERNEL);
		return *out ? 0 : -ENOMEM;
	}
	*n = 0;
	fwnode_for_each_child_node(g, child)
		(*n)++;
	l = devm_kcalloc(dev, max(*n, 1), sizeof(*l), GFP_KERNEL);
	if (!l || *n > 32) {
		fwnode_handle_put(g);
		return l ? dev_err_probe(dev, -EINVAL, "%s: more than 32\n", group) : -ENOMEM;
	}
	fwnode_for_each_child_node(g, child) {
		if (fwnode_property_read_u32(child, "reg", &reg) || reg >= (u32)*n ||
		    l[reg].seen || ohc_read_pins(child, "control4,pin", &l[reg].pin, 1)) {
			fwnode_handle_put(child);
			fwnode_handle_put(g);
			return dev_err_probe(dev, -EINVAL,
					     "%s: each child needs its own reg 0..%d\n",
					     group, *n - 1);
		}
		l[reg].seen = true;
		if (fwnode_property_read_bool(child, "control4,active-low"))
			l[reg].flags |= OHC_CFG_F_ACTIVE_LOW;
		if (fwnode_property_read_bool(child, "bias-pull-up"))
			l[reg].flags |= OHC_CFG_F_PULL_UP;
		if (fwnode_property_read_bool(child, "bias-pull-down"))
			l[reg].flags |= OHC_CFG_F_PULL_DOWN;
	}
	fwnode_handle_put(g);
	*out = l;
	return 0;
}

static bool ohc_is_jack(struct fwnode_handle *child)
{
	const char *name = fwnode_get_name(child);

	return name && str_has_prefix(name, "jack@");
}

/*
 * What sits behind the part. jack@N children carry the panel jacks (each with
 * control4,ir-channel and/or control4,uart, and their pins); without them the
 * counts describe IR-only jacks 1..ir-outputs on channels 0.. and UARTs on
 * connectors of their own. ir-blaster / ir-receiver nodes carry the front
 * emitter and receiver; the boolean properties of the same names say only
 * that they exist. Documentation/devicetree/bindings/gpio/control4,io-mcu.yaml
 */
static int ohc_parse_node(struct device *dev, struct ohc_geometry *geo)
{
	struct fwnode_handle *child;
	struct ohc_jack *j;
	int n = 0, i, ret, max_uart = -1;
	u32 v;

	{
		struct fwnode_handle *node = dev_fwnode(dev);
		u32 v[2 * 16];
		int k, n;

		n = ohc_read_list(node, "control4,ir-carriers", 2, v, OHC_CFG_CARRIERS);
		for (k = 0; k < n; k++)
			geo->carrier[k] = (struct ohc_pin){ .set = true, .port = v[2 * k], .pin = v[2 * k + 1] };
		geo->n_hold = ohc_read_list(node, "control4,hold-low-pins", 2, v, 16);
		for (k = 0; k < geo->n_hold; k++)
			geo->hold[k] = (struct ohc_pin){ .set = true, .port = v[2 * k], .pin = v[2 * k + 1] };
		if (n < 0 || geo->n_hold < 0 ||
		    ohc_read_pins(node, "control4,heartbeat-pin", &geo->heartbeat, 1))
			return dev_err_probe(dev, -EINVAL, "bad carrier, hold or heartbeat pin\n");
		geo->gates_active_low = fwnode_property_read_bool(node, "control4,ir-gates-active-low");
	}

	ret = ohc_parse_lines(dev, "relays", "control4,relays", &geo->relays, &geo->relay);
	if (!ret)
		ret = ohc_parse_lines(dev, "contacts", "control4,contacts", &geo->contacts,
				      &geo->contact);
	if (ret)
		return ret;

	geo->blaster_ch = -1;
	child = device_get_named_child_node(dev, "ir-blaster");
	if (child) {
		geo->ir_blaster = 1;
		if (!fwnode_property_read_u32(child, "control4,ir-channel", &v))
			geo->blaster_ch = v < OHC_IR_MAX_OUTPUTS ? (int)v : -1;
		ret = ohc_read_pins(child, "control4,ir-pin", &geo->blaster_pin, 1);
		geo->n_blaster_gates = ohc_read_gates(child, geo->blaster_gate);
		fwnode_handle_put(child);
		if (ret || geo->n_blaster_gates < 0)
			return dev_err_probe(dev, -EINVAL, "ir-blaster: bad pin\n");
	}
	child = device_get_named_child_node(dev, "ir-receiver");
	if (child) {
		geo->ir_in = 1;
		ret = ohc_read_pins(child, "control4,ir-pin", &geo->rx_pin, 1);
		if (fwnode_property_read_bool(child, "control4,demodulated"))
			geo->rx_flags |= OHC_CFG_F_DEMODULATED;
		if (fwnode_property_read_bool(child, "control4,active-low"))
			geo->rx_flags |= OHC_CFG_F_ACTIVE_LOW;
		fwnode_handle_put(child);
		if (ret)
			return dev_err_probe(dev, ret, "ir-receiver: bad pin\n");
	}

	device_for_each_child_node(dev, child)
		if (ohc_is_jack(child))
			n++;

	if (!n) {
		geo->jacks = devm_kcalloc(dev, max(geo->ir_out, 1), sizeof(*j), GFP_KERNEL);
		if (!geo->jacks)
			return -ENOMEM;
		for (i = 0; i < geo->ir_out; i++)
			geo->jacks[i] = (struct ohc_jack){ .num = i + 1, .ir = i, .uart = -1 };
		geo->n_jacks = geo->ir_out;
		return 0;
	}

	geo->jacks = devm_kcalloc(dev, n, sizeof(*j), GFP_KERNEL);
	if (!geo->jacks)
		return -ENOMEM;
	geo->ir_out = 0;
	j = geo->jacks;
	device_for_each_child_node(dev, child) {
		if (!ohc_is_jack(child))
			continue;
		ret = ohc_parse_jack(dev, child, j);
		if (ret) {
			fwnode_handle_put(child);
			return ret;
		}
		if (j->ir >= 0)
			geo->ir_out++;
		max_uart = max(max_uart, j->uart);
		j++;
	}
	geo->n_jacks = j - geo->jacks;
	geo->uarts = max_uart + 1;
	return 0;
}

/* The front blaster's IROUT_SEND bit: its own, or the one after the last jack. */
static int ohc_blaster_bit(const struct ohc_geometry *geo)
{
	int i, bit = 0;

	if (geo->blaster_ch >= 0)
		return geo->blaster_ch;
	for (i = 0; i < geo->n_jacks; i++)
		bit = max(bit, geo->jacks[i].ir + 1);
	return bit;
}

/*
 * Undo everything ohc_identify registered. The port must still be open: a
 * request in flight needs its reply to finish.
 */
static void ohc_teardown(struct ohc_iomcu *mcu)
{
	int i, lines;

	if (mcu->fwl)
		firmware_upload_unregister(mcu->fwl);
	if (mcu->info)
		device_unregister(mcu->info);
	cancel_delayed_work_sync(&mcu->identify_work);
	cancel_delayed_work_sync(&mcu->fw_work);
	cancel_delayed_work_sync(&mcu->poll_work);
	cancel_delayed_work_sync(&mcu->rx_idle);
	ohc_unregister_serial(mcu);
	for (i = 0; i < mcu->n_emitters; i++) {
		if (mcu->emitters[i].rc)
			rc_unregister_device(mcu->emitters[i].rc);
		kfree(mcu->emitters[i].name);
	}
	kfree(mcu->emitters);
	if (mcu->rx)
		rc_unregister_device(mcu->rx);
	lines = mcu->geo.relays + mcu->geo.contacts;
	ohc_del_nodes(mcu, lines);
	/* ohc_identify may have decided not to register one. */
	if (mcu->chip_added)
		gpiochip_remove(&mcu->gc);

	if (mcu->names) {
		for (i = 0; i < lines; i++)
			kfree(mcu->names[i]);
		kfree(mcu->names);
	}
}

static int ohc_iomcu_probe(struct serdev_device *serdev)
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
	int ret;

	device_property_read_u32(dev, "current-speed", &speed);

	if (geo.relays < 0 || geo.contacts < 0 || geo.ir_out < 0 || geo.uarts < 0)
		return dev_err_probe(dev, -EINVAL, "implausible count in the node\n");
	ret = ohc_parse_node(dev, &geo);
	if (ret)
		return ret;
	/*
	 * A board may have IR and no GPIO lines at all (the EA1), so "no relays
	 * and no contacts" is not a reason to refuse the part — only "nothing at
	 * all behind it" is.
	 */
	if (geo.relays + geo.contacts + geo.ir_out + geo.ir_blaster + geo.ir_in +
	    geo.uarts == 0)
		return dev_err_probe(dev, -EINVAL,
				     "node describes no relays, contacts, IR or serial\n");

	mcu = kzalloc(sizeof(*mcu), GFP_KERNEL);
	if (!mcu)
		return -ENOMEM;

	mcu->serdev = serdev;
	mcu->dev = dev;
	mcu->name = dev_name(dev);
	mcu->geo = geo;
	mutex_init(&mcu->io_lock);
	mutex_init(&mcu->fw_lock);
	spin_lock_init(&mcu->rx_lock);
	init_completion(&mcu->reply);
	init_waitqueue_head(&mcu->raw_wait);
	INIT_DELAYED_WORK(&mcu->poll_work, ohc_poll);
	INIT_DELAYED_WORK(&mcu->identify_work, ohc_identify);
	INIT_DELAYED_WORK(&mcu->fw_work, ohc_fw_work);
	INIT_DELAYED_WORK(&mcu->rx_idle, ohc_ir_idle);

	/*
	 * The part, from the compatible: what an image must be built for.
	 * firmware-name opts the board in to the loader; reset-gpios lets it
	 * replace firmware that has no reset opcode of its own (stock's).
	 */
	mcu->part = (uintptr_t)device_get_match_data(dev);
	device_property_read_string(dev, "firmware-name", &mcu->fw_name);
	if (mcu->fw_name && !mcu->part) {
		dev_warn(dev, "firmware-name needs a part in the compatible; not loading\n");
		mcu->fw_name = NULL;
	}
	mcu->reset = devm_gpiod_get_optional(dev, "reset", GPIOD_OUT_LOW);
	if (IS_ERR(mcu->reset)) {
		ret = PTR_ERR(mcu->reset);
		if (ret == -EPROBE_DEFER) {
			kfree(mcu);
			return ret;
		}
		dev_warn(dev, "cannot take reset-gpios: %d\n", ret);
		mcu->reset = NULL;
	}

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
	mcu->baud = serdev_device_set_baudrate(serdev, speed);
	serdev_device_set_flow_control(serdev, false);
	ret = serdev_device_set_parity(serdev, SERDEV_PARITY_NONE);
	if (ret)
		dev_warn(dev, "cannot clear parity: %d\n", ret);
	if (mcu->baud != speed)
		dev_warn(dev, "asked for %u baud, the UART gave %u\n", speed, mcu->baud);

	/*
	 * No dev_t, so no /dev node: a sysfs directory only. Not fatal if it
	 * fails — the chip still works, iod just cannot see the declaration
	 * (and says so).
	 */
	mcu->info = device_create_with_groups(&ohc_class, dev, 0, mcu,
					      ohc_info_groups, "iomcu");
	if (IS_ERR(mcu->info)) {
		dev_warn(dev, "cannot publish /sys/class/ohc/iomcu: %ld\n",
			 PTR_ERR(mcu->info));
		mcu->info = NULL;
	}

	if (mcu->part) {
		mcu->fwl = firmware_upload_register(THIS_MODULE, dev, "ohc-iomcu",
						    &ohc_up_ops, mcu);
		if (IS_ERR(mcu->fwl)) {
			dev_warn(dev, "no firmware upload: %ld\n", PTR_ERR(mcu->fwl));
			mcu->fwl = NULL;
		}
	}

	/* Bring-up and identify take a second; not in the boot-time probe. */
	schedule_delayed_work(&mcu->identify_work, msecs_to_jiffies(50));
	dev_info(dev, "IO microcontroller at %u baud\n", mcu->baud);
	return 0;
}

static void ohc_iomcu_remove(struct serdev_device *serdev)
{
	struct ohc_iomcu *mcu = serdev_device_get_drvdata(serdev);

	ohc_teardown(mcu);
	serdev_device_close(serdev);
	kfree(mcu);
}

static const struct of_device_id ohc_iomcu_of_match[] = {
	{ .compatible = "control4,tm4c1231d5-io-mcu", .data = (void *)OHC_PART_TM4C1231D5 },
	{ .compatible = "control4,lm3s1162-io-mcu", .data = (void *)OHC_PART_LM3S1162 },
	{ .compatible = "control4,io-mcu" },
	{ }
};
MODULE_DEVICE_TABLE(of, ohc_iomcu_of_match);

static struct serdev_device_driver ohc_iomcu_driver = {
	.probe	= ohc_iomcu_probe,
	.remove	= ohc_iomcu_remove,
	.driver	= {
		.name		= "ohc-iomcu",
		.of_match_table	= ohc_iomcu_of_match,
	},
};

/* The class first: probe publishes into it, and probe can run at register. */
static int __init ohc_iomcu_init(void)
{
	int ret;

	ret = class_register(&ohc_class);
	if (ret) {
		pr_err("cannot register the ohc class: %d\n", ret);
		return ret;
	}
	ret = serdev_device_driver_register(&ohc_iomcu_driver);
	if (ret) {
		pr_err("cannot register the serdev driver: %d\n", ret);
		class_unregister(&ohc_class);
	}
	return ret;
}

static void __exit ohc_iomcu_exit(void)
{
	serdev_device_driver_unregister(&ohc_iomcu_driver);
	class_unregister(&ohc_class);
}

module_init(ohc_iomcu_init);
module_exit(ohc_iomcu_exit);

MODULE_DESCRIPTION("Control4 IO microcontroller relays, contacts, IR and serial as standard devices");
MODULE_AUTHOR("Joseph Telaak");
MODULE_LICENSE("GPL");
