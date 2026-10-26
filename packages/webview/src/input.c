/*
 * input -- USB keyboards and mice for the libwpe display path (the EAs).
 *
 * WPE Platform's DRM display (the HC-800) reads input itself through libinput.
 * A libwpe backend is only told what to draw, and wpebackend-pvr draws straight
 * to /dev/fb0 with no window system behind it, so nothing on that path ever
 * reads a keyboard. This does: it opens every /dev/input/event* that is a
 * keyboard or a pointer, follows hotplug (a keyboard on a hub that is unplugged
 * and plugged back in comes back on its own), and hands the events to WebKit
 * through wpe_view_backend_dispatch_{keyboard,pointer,axis}_event.
 *
 * Keys become keysyms through libxkbcommon, as libwpe expects (key_code is the
 * keysym, hardware_key_code the XKB keycode); the layout is xkbcommon's default
 * (us) unless XKB_DEFAULT_LAYOUT and friends say otherwise. The keymap is built
 * from the XKB data (xkeyboard-config) under XKB_CONFIG_ROOT; without it the
 * keyboard is off and the pointer still works.
 *
 * Devices are not grabbed: anything else on the box that reads the same device
 * (a remapper, a game) keeps getting its events. Game controllers are left
 * alone entirely -- they are not a way to drive a web page.
 */

#include "input.h"

#include <glib.h>
#include <glib-unix.h>
#include <xkbcommon/xkbcommon.h>
#include <linux/input.h>
#include <sys/inotify.h>
#include <sys/ioctl.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define INPUT_DIR "/dev/input"

#define NBITS(n)	(((n) + 8 * sizeof(unsigned long) - 1) / (8 * sizeof(unsigned long)))
#define TEST_BIT(b, a)	(((a)[(b) / (8 * sizeof(unsigned long))] >> ((b) % (8 * sizeof(unsigned long)))) & 1UL)

enum {
	DEV_KEYBOARD = 1 << 0,
	DEV_MOUSE    = 1 << 1,	/* relative: a mouse, a trackball */
	DEV_ABS      = 1 << 2,	/* absolute: a touch panel, a tablet, a KVM's mouse */
};

struct input;

struct device {
	struct input *in;
	char *node;		/* "event3" */
	char *name;
	int fd;
	guint watch;
	unsigned kind;
	int dx, dy;		/* relative motion since the last SYN_REPORT */
	int ax, ay;		/* absolute position, device units */
	gboolean abs_moved;
	struct input_absinfo xinfo, yinfo;
};

struct input {
	struct wpe_view_backend *backend;
	GHashTable *devices;	/* node -> struct device */
	struct xkb_context *xkb;
	struct xkb_keymap *keymap;
	struct xkb_state *state;
	uint32_t width, height;	/* the view, in its own pixels */
	int x, y;		/* the pointer, in view pixels */
	uint32_t buttons;	/* wpe_input_pointer_modifier_button* held */
};

/*
 * The view's size. wpebackend-pvr reports it to WebKit and libwpe gives the
 * application no way to ask, so this reads the same setting the backend does
 * (WPE_PVR_SIZE, else 720x480; board/ea/common/packages/wpebackend-pvr).
 */
static void view_size(uint32_t *w, uint32_t *h)
{
	const char *e = g_getenv("WPE_PVR_SIZE");
	unsigned a, b;

	*w = 720; *h = 480;
	if (e && sscanf(e, "%ux%u", &a, &b) == 2 && a && b) {
		*w = a; *h = b;
	}
}

static uint32_t key_modifiers(struct input *in)
{
	uint32_t m = 0;

	if (!in->state)
		return 0;
	if (xkb_state_mod_name_is_active(in->state, XKB_MOD_NAME_CTRL, XKB_STATE_MODS_EFFECTIVE) > 0)
		m |= wpe_input_keyboard_modifier_control;
	if (xkb_state_mod_name_is_active(in->state, XKB_MOD_NAME_SHIFT, XKB_STATE_MODS_EFFECTIVE) > 0)
		m |= wpe_input_keyboard_modifier_shift;
	if (xkb_state_mod_name_is_active(in->state, XKB_MOD_NAME_ALT, XKB_STATE_MODS_EFFECTIVE) > 0)
		m |= wpe_input_keyboard_modifier_alt;
	if (xkb_state_mod_name_is_active(in->state, XKB_MOD_NAME_LOGO, XKB_STATE_MODS_EFFECTIVE) > 0)
		m |= wpe_input_keyboard_modifier_meta;
	return m;
}

static uint32_t ms(const struct input_event *ev)
{
	return (uint32_t)(ev->input_event_sec * 1000u + ev->input_event_usec / 1000u);
}

/* value: 1 press, 0 release, 2 the kernel's autorepeat. */
static void on_key(struct input *in, const struct input_event *ev)
{
	struct wpe_input_keyboard_event k;
	xkb_keycode_t code = ev->code + 8;	/* evdev -> XKB keycode */

	if (!in->state)
		return;
	memset(&k, 0, sizeof k);
	k.time = ms(ev);
	k.hardware_key_code = code;
	k.pressed = ev->value != 0;
	/* The keysym this key produces in the state before it changes the state:
	 * Shift itself is Shift_L, and the 'a' typed while Shift is held is 'A'. */
	k.key_code = xkb_state_key_get_one_sym(in->state, code);
	if (ev->value != 2)
		xkb_state_update_key(in->state, code, ev->value ? XKB_KEY_DOWN : XKB_KEY_UP);
	k.modifiers = key_modifiers(in);
	if (k.key_code != XKB_KEY_NoSymbol)
		wpe_view_backend_dispatch_keyboard_event(in->backend, &k);
}

static void dispatch_pointer(struct input *in, uint32_t time, enum wpe_input_pointer_event_type type,
			     uint32_t button, uint32_t state)
{
	struct wpe_input_pointer_event p;

	memset(&p, 0, sizeof p);
	p.type = type;
	p.time = time;
	p.x = in->x;
	p.y = in->y;
	p.button = button;
	p.state = state;
	p.modifiers = in->buttons | key_modifiers(in);
	wpe_view_backend_dispatch_pointer_event(in->backend, &p);
}

static void on_button(struct input *in, const struct input_event *ev)
{
	uint32_t button, mask;

	/* WPE numbers buttons 1 left, 2 right, 3 middle -- evdev's order from BTN_LEFT.
	 * A touch panel's contact is a left click. */
	if (ev->code == BTN_TOUCH)
		button = 1;
	else if (ev->code >= BTN_LEFT && ev->code <= BTN_TASK)
		button = ev->code - BTN_LEFT + 1;
	else
		return;
	if (ev->value == 2)
		return;
	mask = button == 1 ? wpe_input_pointer_modifier_button1
	     : button == 2 ? wpe_input_pointer_modifier_button2
	     : button == 3 ? wpe_input_pointer_modifier_button3
	     : button == 4 ? wpe_input_pointer_modifier_button4
	     : button == 5 ? wpe_input_pointer_modifier_button5 : 0;
	if (ev->value)
		in->buttons |= mask;
	else
		in->buttons &= ~mask;
	dispatch_pointer(in, ms(ev), wpe_input_pointer_event_type_button, button, ev->value ? 1 : 0);
}

static void on_wheel(struct input *in, const struct input_event *ev)
{
	struct wpe_input_axis_event a;

	/*
	 * WebKit reads only the sign of a discrete wheel event and scrolls a line
	 * per event. Its axis 0 is vertical with positive meaning up -- evdev's
	 * REL_WHEEL exactly; axis 1 is horizontal with positive meaning left, the
	 * opposite of REL_HWHEEL. The hi-res wheel events that come alongside are
	 * the same motion again and are not used.
	 */
	memset(&a, 0, sizeof a);
	a.type = wpe_input_axis_event_type_motion;
	a.time = ms(ev);
	a.x = in->x;
	a.y = in->y;
	a.axis = ev->code == REL_WHEEL ? 0 : 1;
	a.value = ev->code == REL_WHEEL ? ev->value : -ev->value;
	a.modifiers = in->buttons | key_modifiers(in);
	if (a.value)
		wpe_view_backend_dispatch_axis_event(in->backend, &a);
}

static int clampi(int v, int lo, int hi)
{
	return v < lo ? lo : v > hi ? hi : v;
}

static int scale_abs(int v, const struct input_absinfo *ai, uint32_t span)
{
	long range = (long)ai->maximum - ai->minimum;

	if (range <= 0)
		return 0;
	return (int)(((long)(v - ai->minimum) * (long)(span - 1)) / range);
}

static void on_syn(struct input *in, struct device *d, const struct input_event *ev)
{
	int x = in->x, y = in->y;

	if (d->dx || d->dy) {
		x += d->dx;
		y += d->dy;
		d->dx = d->dy = 0;
	}
	if (d->abs_moved) {
		x = scale_abs(d->ax, &d->xinfo, in->width);
		y = scale_abs(d->ay, &d->yinfo, in->height);
		d->abs_moved = FALSE;
	}
	x = clampi(x, 0, (int)in->width - 1);
	y = clampi(y, 0, (int)in->height - 1);
	if (x == in->x && y == in->y)
		return;
	in->x = x;
	in->y = y;
	dispatch_pointer(in, ms(ev), wpe_input_pointer_event_type_motion, 0, 0);
}

static void handle(struct device *d, const struct input_event *ev)
{
	struct input *in = d->in;

	switch (ev->type) {
	case EV_KEY:
		if (ev->code >= BTN_MISC && ev->code < KEY_OK) {
			if (d->kind & (DEV_MOUSE | DEV_ABS))
				on_button(in, ev);
		} else if (d->kind & DEV_KEYBOARD)
			on_key(in, ev);
		break;
	case EV_REL:
		if (!(d->kind & DEV_MOUSE))
			break;
		if (ev->code == REL_X)
			d->dx += ev->value;
		else if (ev->code == REL_Y)
			d->dy += ev->value;
		else if (ev->code == REL_WHEEL || ev->code == REL_HWHEEL)
			on_wheel(in, ev);
		break;
	case EV_ABS:
		if (!(d->kind & DEV_ABS))
			break;
		if (ev->code == ABS_X) {
			d->ax = ev->value;
			d->abs_moved = TRUE;
		} else if (ev->code == ABS_Y) {
			d->ay = ev->value;
			d->abs_moved = TRUE;
		}
		break;
	case EV_SYN:
		if (ev->code == SYN_REPORT)
			on_syn(in, d, ev);
		break;
	}
}

static void device_close(struct device *d);

static gboolean on_readable(gint fd, GIOCondition cond, gpointer data)
{
	struct device *d = data;
	struct input_event ev[64];
	ssize_t n;

	for (;;) {
		n = read(fd, ev, sizeof ev);
		if (n < 0 && errno == EINTR)
			continue;
		if (n < 0 && errno == EAGAIN)
			break;
		if (n <= 0) {
			/* ENODEV: unplugged. Forget it; the node coming back is a new device. */
			g_printerr("webview: input: %s (%s) gone\n", d->node, d->name);
			d->watch = 0;
			g_hash_table_remove(d->in->devices, d->node);
			return G_SOURCE_REMOVE;
		}
		for (size_t i = 0; i < (size_t)n / sizeof ev[0]; i++)
			handle(d, &ev[i]);
	}
	if (cond & (G_IO_HUP | G_IO_ERR)) {
		d->watch = 0;
		g_hash_table_remove(d->in->devices, d->node);
		return G_SOURCE_REMOVE;
	}
	return G_SOURCE_CONTINUE;
}

static void device_close(struct device *d)
{
	struct input *in = d->in;

	if (d->watch)
		g_source_remove(d->watch);
	close(d->fd);
	/*
	 * A keyboard unplugged with Shift held never sends the release, and the
	 * shared state would keep typing capitals; a mouse unplugged mid-drag would
	 * keep dragging. Start both over.
	 */
	if ((d->kind & DEV_KEYBOARD) && in->keymap) {
		xkb_state_unref(in->state);
		in->state = xkb_state_new(in->keymap);
	}
	if (d->kind & (DEV_MOUSE | DEV_ABS))
		in->buttons = 0;
	g_free(d->node);
	g_free(d->name);
	g_free(d);
}

/* What this device is to a web page: a keyboard, a pointer, both, or nothing. */
static unsigned classify(int fd)
{
	unsigned long ev[NBITS(EV_MAX + 1)] = { 0 };
	unsigned long key[NBITS(KEY_MAX + 1)] = { 0 };
	unsigned long rel[NBITS(REL_MAX + 1)] = { 0 };
	unsigned long abs[NBITS(ABS_MAX + 1)] = { 0 };
	unsigned kind = 0;

	if (ioctl(fd, EVIOCGBIT(0, sizeof ev), ev) < 0)
		return 0;
	if (TEST_BIT(EV_KEY, ev))
		ioctl(fd, EVIOCGBIT(EV_KEY, sizeof key), key);
	if (TEST_BIT(EV_REL, ev))
		ioctl(fd, EVIOCGBIT(EV_REL, sizeof rel), rel);
	if (TEST_BIT(EV_ABS, ev))
		ioctl(fd, EVIOCGBIT(EV_ABS, sizeof abs), abs);

	/* Game controllers: sticks and pads are not a pointer, their buttons not keys. */
	if (TEST_BIT(BTN_JOYSTICK, key) || TEST_BIT(BTN_GAMEPAD, key))
		return 0;

	/* Anything with ordinary keys (letters, Enter, the arrows) types into the
	 * page; a lone power or sleep button does not count. */
	for (unsigned k = KEY_ESC; k <= KEY_KPDOT && !kind; k++)
		if (TEST_BIT(k, key))
			kind |= DEV_KEYBOARD;
	if (TEST_BIT(KEY_UP, key) || TEST_BIT(KEY_DOWN, key) || TEST_BIT(KEY_ENTER, key))
		kind |= DEV_KEYBOARD;

	if (TEST_BIT(REL_X, rel) && TEST_BIT(REL_Y, rel) && TEST_BIT(BTN_LEFT, key))
		kind |= DEV_MOUSE;
	if (TEST_BIT(ABS_X, abs) && TEST_BIT(ABS_Y, abs) &&
	    (TEST_BIT(BTN_TOUCH, key) || TEST_BIT(BTN_LEFT, key)))
		kind |= DEV_ABS;
	return kind;
}

static void device_open(struct input *in, const char *node)
{
	char path[PATH_MAX];
	char name[256] = "?";
	struct device *d;
	unsigned kind;
	int fd;

	if (!g_str_has_prefix(node, "event") || g_hash_table_contains(in->devices, node))
		return;
	snprintf(path, sizeof path, INPUT_DIR "/%s", node);
	fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
	if (fd < 0)
		return;		/* not ready yet; IN_ATTRIB brings it back once udev is done */
	ioctl(fd, EVIOCGNAME(sizeof name - 1), name);
	kind = classify(fd);
	if (!kind) {
		close(fd);
		return;
	}

	d = g_new0(struct device, 1);
	d->in = in;
	d->node = g_strdup(node);
	d->name = g_strdup(name);
	d->fd = fd;
	d->kind = kind;
	if (kind & DEV_ABS) {
		ioctl(fd, EVIOCGABS(ABS_X), &d->xinfo);
		ioctl(fd, EVIOCGABS(ABS_Y), &d->yinfo);
	}
	d->watch = g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR, on_readable, d);
	g_hash_table_insert(in->devices, d->node, d);
	g_printerr("webview: input: %s \"%s\"%s%s%s\n", node, name,
		   kind & DEV_KEYBOARD ? " keyboard" : "",
		   kind & DEV_MOUSE ? " mouse" : "",
		   kind & DEV_ABS ? " absolute-pointer" : "");
}

static void scan(struct input *in)
{
	GDir *dir = g_dir_open(INPUT_DIR, 0, NULL);
	const char *n;

	if (!dir)
		return;
	while ((n = g_dir_read_name(dir)))
		device_open(in, n);
	g_dir_close(dir);
}

/*
 * Hotplug: watch /dev/input itself rather than talk to udev. A node appears
 * (IN_CREATE) as soon as the kernel registers the device; IN_ATTRIB follows
 * when udev has set it up, which is the retry if the first open was too early.
 * Removal needs no watch: the open fd reports ENODEV.
 */
static gboolean on_inotify(gint fd, GIOCondition cond, gpointer data)
{
	struct input *in = data;
	char buf[4096] __attribute__((aligned(__alignof__(struct inotify_event))));
	ssize_t n;
	(void)cond;

	while ((n = read(fd, buf, sizeof buf)) > 0) {
		for (char *p = buf; p < buf + n; ) {
			struct inotify_event *e = (struct inotify_event *)p;
			if (e->mask & IN_Q_OVERFLOW)
				scan(in);
			else if (e->len)
				device_open(in, e->name);
			p += sizeof *e + e->len;
		}
	}
	return G_SOURCE_CONTINUE;
}

void input_start(struct wpe_view_backend *backend)
{
	struct input *in = g_new0(struct input, 1);
	int ifd;

	in->backend = backend;
	in->devices = g_hash_table_new_full(g_str_hash, g_str_equal, NULL,
					    (GDestroyNotify)device_close);
	view_size(&in->width, &in->height);
	in->x = (int)in->width / 2;
	in->y = (int)in->height / 2;

	in->xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	if (in->xkb)
		in->keymap = xkb_keymap_new_from_names(in->xkb, NULL, XKB_KEYMAP_COMPILE_NO_FLAGS);
	if (in->keymap)
		in->state = xkb_state_new(in->keymap);
	else
		g_printerr("webview: input: no XKB keymap (is xkeyboard-config under "
			   "XKB_CONFIG_ROOT?) -- keyboards are off, pointers still work\n");

	ifd = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
	if (ifd < 0 || inotify_add_watch(ifd, INPUT_DIR, IN_CREATE | IN_ATTRIB) < 0)
		g_printerr("webview: input: cannot watch %s (%s) -- no hotplug\n",
			   INPUT_DIR, g_strerror(errno));
	else
		g_unix_fd_add(ifd, G_IO_IN, on_inotify, in);
	scan(in);
}
