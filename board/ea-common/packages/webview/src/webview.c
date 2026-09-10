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
	 */
	webkit_settings_set_hardware_acceleration_policy(settings,
		g_getenv("WEBVIEW_NOACCEL")
			? WEBKIT_HARDWARE_ACCELERATION_POLICY_NEVER
			: WEBKIT_HARDWARE_ACCELERATION_POLICY_ALWAYS);
	/* An appliance surface has no user to right-click and no keyboard
	 * shortcut to undo an accidental selection. */
	webkit_settings_set_enable_developer_extras(settings, FALSE);

	g_signal_connect(view, "load-failed", G_CALLBACK(on_load_failed), NULL);
	webkit_web_view_load_uri(view, url);

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
