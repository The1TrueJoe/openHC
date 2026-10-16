/*
 * webview -- a full-screen web surface for openHC.
 *
 * WPE WebKit ships no browser of its own (Buildroot builds it with
 * ENABLE_MINIBROWSER=OFF, and a MiniBrowser is a developer tool regardless),
 * so something has to create the view and run a main loop. That is all this
 * is. There is no chrome, no navigation, no tabs: the box shows one page.
 *
 * The display path is entirely in the libwpe backend, not here --
 * wpe_view_backend_create() dlopens it, and on this hardware that is
 * wpebackend-pvr, whose renderer draws through the PowerVR DDK's LinuxFB
 * WSEGL straight to /dev/fb0. So there is no window system to talk to and
 * nothing here to configure about the display.
 */

#include <wpe/webkit.h>
#include <wpe/wpe.h>
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

static gboolean on_load_failed(WebKitWebView *view, WebKitLoadEvent ev,
			       const char *uri, GError *err, gpointer user)
{
	(void)view; (void)ev; (void)user;
	/*
	 * A launcher that fails to load its page must not sit on a blank
	 * screen: on an appliance there is no address bar to tell you what
	 * went wrong. Say so on the console and keep running, because the
	 * usual cause is that whatever serves the page has not finished
	 * starting yet and the reload below will succeed.
	 */
	g_printerr("webview: load failed: %s: %s\n", uri,
		   err ? err->message : "unknown error");
	return FALSE;
}

static gboolean reload_cb(gpointer view)
{
	webkit_web_view_reload((WebKitWebView *)view);
	return G_SOURCE_CONTINUE;
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
struct ctl { struct wpe_view_backend *backend; WebKitWebView *view; };

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

static void dispatch_key(struct wpe_view_backend *b, uint32_t keysym)
{
	struct wpe_input_keyboard_event ev;
	if (!keysym) return;
	memset(&ev, 0, sizeof ev);
	ev.key_code = keysym;
	ev.pressed = true;
	wpe_view_backend_dispatch_keyboard_event(b, &ev);
	ev.pressed = false;
	wpe_view_backend_dispatch_keyboard_event(b, &ev);
}

/*
 * Day/night: the color scheme pages see (prefers-color-scheme, the color-scheme
 * CSS property, native controls), through WebKit's per-view dark mode API
 * (board/common/patches/wpewebkit/0001). "auto" (or unset) follows the system
 * setting, which on this panel has no source and so stays light.
 */
static void set_color_scheme(WebKitWebView *view, const char *scheme)
{
	if (!scheme || !*scheme || g_strcmp0(scheme, "auto") == 0)
		webkit_web_view_unset_prefers_dark_mode(view);
	else if (g_strcmp0(scheme, "light") == 0 || g_strcmp0(scheme, "dark") == 0)
		webkit_web_view_set_prefers_dark_mode(view, g_strcmp0(scheme, "dark") == 0);
	else
		g_printerr("webview: unknown theme '%s' (auto|light|dark)\n", scheme);
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
			dispatch_key(c->backend, keysym_for(line + 4));
		else if (g_str_has_prefix(line, "text "))
			for (const char *p = line + 5; *p; p++)
				dispatch_key(c->backend, (uint32_t)(unsigned char)*p);
		else if (g_str_has_prefix(line, "url "))
			webkit_web_view_load_uri(c->view, line + 4);
		else if (g_strcmp0(line, "reload") == 0)
			webkit_web_view_reload(c->view);
		else if (g_str_has_prefix(line, "theme "))
			set_color_scheme(c->view, line + 6);
		else if (*line)
			g_printerr("webview: unknown control line: %s\n", line);
		g_free(line);
		line = NULL;
	}
	return TRUE;	/* keep the watch armed */
}

static void control_open(struct wpe_view_backend *backend, WebKitWebView *view)
{
	const char *path = g_getenv("OHC_DISPLAY_CTL");
	int fd;
	struct ctl *c;
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
	c = g_new(struct ctl, 1);
	c->backend = backend;
	c->view = view;
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
	guint retry_s = 0;

	if (argc > 1)
		url = argv[1];
	if (!url)
		url = g_getenv("WEBVIEW_URL");
	if (!url || !*url)
		url = DEFAULT_URL;

	/*
	 * wpe_view_backend_create() is what loads the libwpe backend. If no
	 * backend can be found it returns NULL rather than aborting, and
	 * every later call would crash on it, so check.
	 */
	struct wpe_view_backend *backend = wpe_view_backend_create();
	if (!backend) {
		g_printerr("webview: no libwpe backend "
			   "(set WPE_BACKEND, or install "
			   "libWPEBackend-default.so.1)\n");
		return 1;
	}

	WebKitWebView *view =
		webkit_web_view_new(webkit_web_view_backend_new(backend, NULL, NULL));

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
	set_color_scheme(view, g_getenv("OHC_DISPLAY_THEME"));

	g_signal_connect(view, "load-failed", G_CALLBACK(on_load_failed), NULL);
	webkit_web_view_load_uri(view, url);

	/* Take commands from outside — MQTT/app/IR via the FIFO (see control_open). */
	control_open(backend, view);

	GMainLoop *loop = g_main_loop_new(NULL, FALSE);
	g_unix_signal_add(SIGINT,  on_signal, loop);
	g_unix_signal_add(SIGTERM, on_signal, loop);

	/*
	 * Optional periodic reload. The page this box shows is served by
	 * something else on the network, which may be rebooting, upgrading or
	 * not up yet at the moment we start. Without this a transient failure
	 * at boot is permanent until someone restarts the service.
	 */
	retry_env = g_getenv("WEBVIEW_RETRY");
	if (retry_env)
		retry_s = (guint)strtoul(retry_env, NULL, 10);
	if (retry_s)
		g_timeout_add_seconds(retry_s, reload_cb, view);

	g_printerr("webview: %s\n", url);
	g_main_loop_run(loop);

	g_main_loop_unref(loop);
	return 0;
}
