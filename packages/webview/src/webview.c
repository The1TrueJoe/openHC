/*
 * webview -- a full-screen web surface for openHC.
 *
 * WPE WebKit ships no browser of its own (Buildroot builds it with
 * ENABLE_MINIBROWSER=OFF, and a MiniBrowser is a developer tool regardless),
 * so something has to create the view and run a main loop. That is all this
 * is. There is no chrome, no navigation, no tabs: the box shows one page.
 *
 * The display path is not here either. It is one of two, chosen at build time
 * by what WebKit was built with (the Makefile defines OHC_WPE_PLATFORM when
 * WebKit ships the WPE Platform API):
 *
 *  - WPE Platform (HC-800): WebKit renders through a WPEDisplay -- its own DRM
 *    platform, which scans the GPU's buffers out on KMS and reads input through
 *    libinput. WPE_DISPLAY picks it (wpe-display-drm).
 *
 *  - libwpe (the EAs): wpe_view_backend_create() dlopens a libwpe backend, on
 *    that hardware wpebackend-pvr, whose renderer draws through the PowerVR
 *    DDK's LinuxFB WSEGL straight to /dev/fb0. The SGX545's closed driver has
 *    nothing WPE Platform can use without copying every frame back off the GPU,
 *    so the EAs stay on this path (openHC issue #55).
 *
 * Either way there is no window system to talk to and nothing to configure here
 * about the display.
 */

#include <wpe/webkit.h>
#ifdef OHC_WPE_PLATFORM
#include <wpe/wpe-platform.h>
#else
#include <wpe/wpe.h>
#endif
#include <glib.h>
#include <glib-object.h>
#include <glib-unix.h>
#include <stdlib.h>
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>

/*
 * The box already serves a UI: webd listens on :80 (S90ohcweb) and
 * serves the React dashboard compiled into it. That is the thing a
 * launcher should show when nobody has said otherwise, so default to it
 * rather than to a blank page. WEBVIEW_URL overrides, which is how a
 * layer built on top of openHC points this somewhere else.
 */
#define DEFAULT_URL "http://localhost/"

static gboolean on_signal(gpointer loop)
{
	g_main_loop_quit((GMainLoop *)loop);
	return G_SOURCE_REMOVE;
}

static guint retry_s;	/* WEBVIEW_RETRY: retry a failed load after this many seconds */

static gboolean reload_cb(gpointer view)
{
	webkit_web_view_reload((WebKitWebView *)view);
	return G_SOURCE_REMOVE;
}

static gboolean on_load_failed(WebKitWebView *view, WebKitLoadEvent ev,
			       const char *uri, GError *err, gpointer user)
{
	(void)ev; (void)user;
	/* Navigating away cancels the load in flight; that is not a failure. */
	if (err && g_error_matches(err, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED))
		return FALSE;
	/*
	 * A launcher that fails to load its page must not sit on a blank
	 * screen: on an appliance there is no address bar to tell you what
	 * went wrong. Say so on the console and keep running, because the
	 * usual cause is that whatever serves the page has not finished
	 * starting yet and a retry will succeed.
	 */
	g_printerr("webview: load failed: %s: %s\n", uri,
		   err ? err->message : "unknown error");
	if (retry_s)
		g_timeout_add_seconds(retry_s, reload_cb, view);
	return FALSE;
}

/* ── control channel: drive the view from outside (MQTT, an app, an IR remote) ──
 *
 * openHC exposes the hardware; a control system on top decides what to show and
 * what the buttons do. So the on-panel view takes commands on a FIFO (default
 * /run/ohc-display.ctl, OHC_DISPLAY_CTL overrides), one line each:
 *
 *   key <Name>     a keypress: Up/Down/Left/Right/Enter/Back/Home/Tab/digits/a char
 *   text <string>  type a run of characters
 *   url <uri>      load a page
 *   reload         reload the current page
 *   theme <scheme> auto|light|dark: the color scheme pages see (live)
 *
 * A bridge (e.g. an MQTT subscriber on <base>/cmd/display/key) just echoes a line
 * into the FIFO; no transport lives here. Keys become real WPE keyboard events
 * dispatched into WebKit, so the page sees ordinary key input.
 */
struct ctl {
	WebKitWebView *view;
#ifdef OHC_WPE_PLATFORM
	WPEView *wpe;		/* key events go to the platform view */
	WPEDisplay *display;	/* its settings carry dark mode */
#else
	struct wpe_view_backend *backend;
#endif
};

/* XKB/WPE keysyms for the names a remote sends; a printable ASCII char maps to itself. */
static uint32_t keysym_for(const char *name)
{
	static const struct { const char *n; uint32_t k; } tab[] = {
		{"Up",0xff52},{"Down",0xff54},{"Left",0xff51},{"Right",0xff53},
		{"Enter",0xff0d},{"Return",0xff0d},{"OK",0xff0d},{"Select",0xff0d},
		{"Back",0xff1b},{"Escape",0xff1b},{"Esc",0xff1b},
		{"Home",0xff50},{"End",0xff57},{"Tab",0xff09},{"Menu",0xff67},
		{"BackSpace",0xff08},{"Delete",0xffff},{"Space",0x0020},
		{"PageUp",0xff55},{"PageDown",0xff56},
	};
	for (size_t i = 0; i < sizeof(tab) / sizeof(tab[0]); i++)
		if (g_ascii_strcasecmp(name, tab[i].n) == 0) return tab[i].k;
	if (name[0] && !name[1] &&
	    (unsigned char)name[0] >= 0x20 && (unsigned char)name[0] < 0x7f)
		return (uint32_t)(unsigned char)name[0];	/* Latin-1 keysym == the char */
	return 0;
}

static void dispatch_key(struct ctl *c, uint32_t keysym)
{
	if (!keysym) return;
#ifdef OHC_WPE_PLATFORM
	static const WPEEventType type[] = { WPE_EVENT_KEYBOARD_KEY_DOWN, WPE_EVENT_KEYBOARD_KEY_UP };
	if (!c->wpe) return;
	for (size_t i = 0; i < 2; i++) {
		WPEEvent *ev = wpe_event_keyboard_new(type[i], c->wpe, WPE_INPUT_SOURCE_KEYBOARD,
						      0, (WPEModifiers)0, 0, keysym);
		wpe_view_event(c->wpe, ev);
		wpe_event_unref(ev);
	}
#else
	struct wpe_input_keyboard_event ev;
	memset(&ev, 0, sizeof ev);
	ev.key_code = keysym;
	ev.pressed = true;
	wpe_view_backend_dispatch_keyboard_event(c->backend, &ev);
	ev.pressed = false;
	wpe_view_backend_dispatch_keyboard_event(c->backend, &ev);
#endif
}

/*
 * Day/night: the color scheme pages see (prefers-color-scheme, the color-scheme
 * CSS property, native controls), applied live. "auto" (or unset) leaves pages
 * at their default, light: these screens have no OS dark mode to follow.
 *
 *  - WPE Platform: the display's own dark-mode setting (WPE_SETTING_DARK_MODE),
 *    upstream API -- like an OS setting.
 *  - libwpe: that path has no settings source, so WebKit's per-view override
 *    added by the EA's wpewebkit patch (board/ea/common/gpu-wpe-i386/patches).
 */
static void set_color_scheme(struct ctl *c, const char *scheme)
{
	gboolean dark;
	if (!scheme || !*scheme || g_strcmp0(scheme, "auto") == 0)
		scheme = "auto";
	else if (g_strcmp0(scheme, "light") != 0 && g_strcmp0(scheme, "dark") != 0) {
		g_printerr("webview: unknown theme '%s' (auto|light|dark)\n", scheme);
		return;
	}
	dark = g_strcmp0(scheme, "dark") == 0;
#ifdef OHC_WPE_PLATFORM
	GError *err = NULL;
	if (!wpe_settings_set_boolean(wpe_display_get_settings(c->display), WPE_SETTING_DARK_MODE,
				      dark, WPE_SETTINGS_SOURCE_APPLICATION, &err)) {
		g_printerr("webview: cannot set dark mode: %s\n", err ? err->message : "?");
		g_clear_error(&err);
	}
#else
	if (g_strcmp0(scheme, "auto") == 0)
		webkit_web_view_unset_prefers_dark_mode(c->view);
	else
		webkit_web_view_set_prefers_dark_mode(c->view, dark);
#endif
}

static gboolean on_ctl(GIOChannel *src, GIOCondition cond, gpointer data)
{
	struct ctl *c = data;
	char *line = NULL;
	gsize len;
	(void)cond;
	while (g_io_channel_read_line(src, &line, &len, NULL, NULL) == G_IO_STATUS_NORMAL) {
		g_strchomp(line);
		if (g_str_has_prefix(line, "key "))
			dispatch_key(c, keysym_for(line + 4));
		else if (g_str_has_prefix(line, "text "))
			for (const char *p = line + 5; *p; p++)
				dispatch_key(c, (uint32_t)(unsigned char)*p);
		else if (g_str_has_prefix(line, "url "))
			webkit_web_view_load_uri(c->view, line + 4);
		else if (g_strcmp0(line, "reload") == 0)
			webkit_web_view_reload(c->view);
		else if (g_str_has_prefix(line, "theme "))
			set_color_scheme(c, line + 6);
		else if (*line)
			g_printerr("webview: unknown control line: %s\n", line);
		g_free(line);
		line = NULL;
	}
	return TRUE;	/* keep the watch armed */
}

static void control_open(struct ctl *c)
{
	const char *path = g_getenv("OHC_DISPLAY_CTL");
	int fd;
	GIOChannel *ch;

	if (!path || !*path) path = "/run/ohc-display.ctl";
	if (mkfifo(path, 0600) != 0 && errno != EEXIST) {
		g_printerr("webview: no control FIFO at %s (%s)\n", path, g_strerror(errno));
		return;
	}
	/* O_RDWR so we always hold a writer too: the channel then never hits EOF when
	 * an external writer closes, so one watch serves every command for the run. */
	fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
	if (fd < 0) {
		g_printerr("webview: cannot open %s (%s)\n", path, g_strerror(errno));
		return;
	}
	ch = g_io_channel_unix_new(fd);
	g_io_channel_set_flags(ch, G_IO_FLAG_NONBLOCK, NULL);
	g_io_channel_set_close_on_unref(ch, TRUE);
	g_io_add_watch(ch, G_IO_IN, on_ctl, c);
	g_io_channel_unref(ch);
	g_printerr("webview: control channel on %s (key/text/url/reload/theme)\n", path);
}

int main(int argc, char **argv)
{
	const char *url = NULL;
	const char *retry_env;

	if (argc > 1)
		url = argv[1];
	if (!url)
		url = g_getenv("WEBVIEW_URL");
	if (!url || !*url)
		url = DEFAULT_URL;

	struct ctl *c = g_new0(struct ctl, 1);
#ifdef OHC_WPE_PLATFORM
	/*
	 * The platform display: WPE_DISPLAY names it (wpe-display-drm here), or
	 * WebKit tries each platform it has. NULL when none connects.
	 */
	c->display = wpe_display_get_default();
	if (!c->display) {
		g_printerr("webview: no WPE Platform display "
			   "(set WPE_DISPLAY, e.g. wpe-display-drm)\n");
		return 1;
	}
	WebKitWebView *view = WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW,
		"display", c->display, NULL));
	c->wpe = webkit_web_view_get_wpe_view(view);
	if (c->wpe && wpe_view_get_toplevel(c->wpe))
		wpe_toplevel_fullscreen(wpe_view_get_toplevel(c->wpe));
#else
	/*
	 * wpe_view_backend_create() is what loads the libwpe backend. If no
	 * backend can be found it returns NULL rather than aborting, and
	 * every later call would crash on it, so check.
	 */
	c->backend = wpe_view_backend_create();
	if (!c->backend) {
		g_printerr("webview: no libwpe backend "
			   "(set WPE_BACKEND, or install "
			   "libWPEBackend-default.so.1)\n");
		return 1;
	}

	WebKitWebView *view =
		webkit_web_view_new(webkit_web_view_backend_new(c->backend, NULL, NULL));
#endif
	c->view = view;

	WebKitSettings *settings = webkit_web_view_get_settings(view);
#if !WEBKIT_CHECK_VERSION(2, 50, 0)
	/*
	 * ALWAYS, not ON_DEMAND: on this GPU the whole point is to keep
	 * compositing off the Atom, and WebKit's heuristics for "does this
	 * page need acceleration" are tuned for desktops where the CPU can
	 * absorb the work. Here it cannot.
	 *
	 * WEBVIEW_NOACCEL=1 forces it off. That is a bring-up escape
	 * hatch, not a supported mode: if a page renders with it and not
	 * without, the fault is in the EGL path rather than in the page, which
	 * is worth being able to establish in one run instead of a rebuild.
	 *
	 * Gone in WPE WebKit 2.50: webkit_settings_set_hardware_acceleration_policy
	 * and the WEBKIT_HARDWARE_ACCELERATION_POLICY_* enum were removed (2.50
	 * always renders through the GPU — exactly what ALWAYS asked for), so on
	 * 2.50+ the default already does the right thing and there is nothing to
	 * call. The WEBVIEW_NOACCEL bring-up hatch has no 2.50 equivalent; it
	 * applies only to the older WebKit this guard still compiles against.
	 */
	webkit_settings_set_hardware_acceleration_policy(settings,
		g_getenv("WEBVIEW_NOACCEL")
			? WEBKIT_HARDWARE_ACCELERATION_POLICY_NEVER
			: WEBKIT_HARDWARE_ACCELERATION_POLICY_ALWAYS);
#endif
	/* An appliance surface has no user to right-click and no keyboard
	 * shortcut to undo an accidental selection. */
	webkit_settings_set_enable_developer_extras(settings, FALSE);

	/*
	 * Force an OPAQUE base colour. A WPE WebKitWebView defaults to a
	 * transparent background; WebKit's AcceleratedSurface then treats the
	 * surface as non-opaque, asks the compositor for an alpha EGL config and
	 * clears every frame to (0,0,0,0). On this board there is nothing behind
	 * the view -- it draws straight to /dev/fb0 through the LinuxFB WSEGL,
	 * which has no alpha-capable window config -- so a non-opaque surface
	 * never lands on the panel (the WebProcess renders, "swaps", and fb0 stays
	 * black). An appliance surface is fully opaque anyway; assert it so the
	 * framebuffer window surface is the real, presentable one.
	 */
	{
		WebKitColor opaque;
		if (webkit_color_parse(&opaque, "#000000"))
			webkit_web_view_set_background_color(view, &opaque);
	}

	/* The starting day/night setting from the display service (S95ohc-display);
	 * the control channel's "theme" changes it live. */
	set_color_scheme(c, g_getenv("OHC_DISPLAY_THEME"));

	/*
	 * Retry a failed load. The page this box shows is served by something
	 * else (webd, or anything on the network), which may be rebooting,
	 * upgrading or not up yet at the moment we start. Without this a
	 * transient failure at boot is permanent until someone restarts the
	 * service. Only on failure: a page that loaded is left alone.
	 */
	retry_env = g_getenv("WEBVIEW_RETRY");
	if (retry_env)
		retry_s = (guint)strtoul(retry_env, NULL, 10);

	g_signal_connect(view, "load-failed", G_CALLBACK(on_load_failed), NULL);
	webkit_web_view_load_uri(view, url);

	/* Take commands from outside — MQTT/app/IR via the FIFO (see control_open). */
	control_open(c);

	GMainLoop *loop = g_main_loop_new(NULL, FALSE);
	g_unix_signal_add(SIGINT,  on_signal, loop);
	g_unix_signal_add(SIGTERM, on_signal, loop);

	g_printerr("webview: %s\n", url);
	g_main_loop_run(loop);

	g_main_loop_unref(loop);
	return 0;
}
