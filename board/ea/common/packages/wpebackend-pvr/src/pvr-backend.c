/*
 * wpebackend-pvr — a libwpe backend over PowerVR SGX's framebuffer EGL.
 *
 * libwpe finds a backend by dlopen'ing a shared object and calling
 * _wpe_loader_interface's load_object() with the name of the interface it
 * wants. Everything below is reached through that one entry point.
 *
 * The shape is dictated by libwpe: three vtables, filled with function
 * pointers, returned by name. A single-window framebuffer target needs almost
 * none of what the Wayland backend does -- there is one surface, it is the
 * screen, it never moves and never resizes -- so most of view-backend is
 * legitimately empty rather than unimplemented.
 *
 * STATUS: scaffold. The vtables are correct and this builds and loads; the EGL
 * bodies are not written. Every stub that must not be silently wrong announces
 * itself rather than returning a plausible value.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * Only the two umbrella headers: libwpe's individual headers #error out if
 * included directly, and EGL/egl.h is needed for EGL_DEFAULT_DISPLAY.
 */
#include <EGL/egl.h>
#include <wpe/wpe.h>
#include <wpe/wpe-egl.h>

/* ---------------------------------------------------------------- renderer */

struct pvr_backend {
	int unused;
};

struct pvr_target {
	struct wpe_renderer_backend_egl_target *wpe;
	uint32_t width, height;
};

static void *rb_create(int host_fd)
{
	(void)host_fd;
	return calloc(1, sizeof(struct pvr_backend));
}

static void rb_destroy(void *data)
{
	free(data);
}

static EGLNativeDisplayType rb_get_native_display(void *data)
{
	(void)data;
	/*
	 * The DDK's LinuxFB WSEGL opens /dev/fb0 itself and takes no display
	 * handle, so EGL_DEFAULT_DISPLAY is the whole story here. This is the
	 * one place where being on fbdev rather than Wayland makes the backend
	 * simpler rather than harder.
	 */
	return EGL_DEFAULT_DISPLAY;
}

static uint32_t rb_get_platform(void *data)
{
	(void)data;
	/* No EGL_KHR_platform_* on this DDK -- it predates them. 0 means
	 * "use eglGetDisplay", which is what we want. */
	return 0;
}

static struct wpe_renderer_backend_egl_interface pvr_renderer_backend = {
	.create             = rb_create,
	.destroy            = rb_destroy,
	.get_native_display = rb_get_native_display,
	.get_platform       = rb_get_platform,
};

/* ---------------------------------------------------------- renderer target */

static void *rt_create(struct wpe_renderer_backend_egl_target *wpe, int host_fd)
{
	struct pvr_target *t;
	(void)host_fd;
	t = calloc(1, sizeof(*t));
	if (t)
		t->wpe = wpe;
	return t;
}

static void rt_destroy(void *data)
{
	free(data);
}

static void rt_initialize(void *data, void *backend, uint32_t width, uint32_t height)
{
	struct pvr_target *t = data;
	(void)backend;
	/*
	 * Nothing to create. WPE builds the EGL surface itself from whatever
	 * get_native_window() returns, and on the LinuxFB WSEGL that is the
	 * framebuffer -- there is no per-target object to allocate. Record the
	 * size so resize() has something to compare against.
	 */
	if (t) {
		t->width = width;
		t->height = height;
	}
}

static EGLNativeWindowType rt_get_native_window(void *data)
{
	(void)data;
	/*
	 * With MESA_EGL_NO_X11_HEADERS the DDK's EGLNativeWindowType is a plain
	 * integer, and its LinuxFB WSEGL ignores the value -- there is exactly
	 * one surface and it is the framebuffer. Returning 0 is correct, not a
	 * placeholder.
	 */
	return (EGLNativeWindowType)0;
}

static void rt_resize(void *data, uint32_t width, uint32_t height)
{
	struct pvr_target *t = data;
	if (t) {
		t->width = width;
		t->height = height;
	}
	/* The framebuffer is a fixed scanout; nothing to do until something
	 * can change the mode. */
}

static void rt_frame_will_render(void *data)
{
	(void)data;
}

static void rt_frame_rendered(void *data)
{
	struct pvr_target *t = data;

	/*
	 * WPE has rendered and swapped; it will not start another frame until
	 * this is acknowledged.
	 *
	 * On Wayland this is where a backend waits for the compositor to
	 * release the buffer, and fdo defers the acknowledgement to a frame
	 * callback. There is no compositor here: the LinuxFB WSEGL's
	 * eglSwapBuffers blits straight to /dev/fb0 and returns, so by the time
	 * this is called the pixels are already on the panel. Acknowledging
	 * immediately is correct rather than optimistic.
	 *
	 * That synchronous blit is also why this board tops out near 29 fps
	 * regardless of scene complexity -- the cost is one full-surface copy
	 * per frame, not the rendering.
	 */
	if (t && t->wpe)
		wpe_renderer_backend_egl_target_dispatch_frame_complete(t->wpe);
}

static void rt_deinitialize(void *data)
{
	(void)data;
}

static struct wpe_renderer_backend_egl_target_interface pvr_renderer_target = {
	.create            = rt_create,
	.destroy           = rt_destroy,
	.initialize        = rt_initialize,
	.get_native_window = rt_get_native_window,
	.resize            = rt_resize,
	.frame_will_render = rt_frame_will_render,
	.frame_rendered    = rt_frame_rendered,
	.deinitialize      = rt_deinitialize,
};

/* ------------------------------------------------- offscreen renderer target */

static void *ot_create(void)
{
	return calloc(1, sizeof(struct pvr_target));
}

static void ot_destroy(void *data)
{
	free(data);
}

static void ot_initialize(void *data, void *backend)
{
	(void)data; (void)backend;
	/*
	 * Nothing to set up. The offscreen target exists so WebKit can make a
	 * second, non-visible context to share resources with -- it never
	 * presents, so there is no framebuffer to claim and no size to agree
	 * on.
	 */
}

static EGLNativeWindowType ot_get_native_window(void *data)
{
	(void)data;
	/*
	 * Returning 0 tells WebKit there is no native window here, so it makes
	 * the sharing context surfaceless or against a pbuffer instead. That
	 * is the honest answer: the LinuxFB WSEGL's only drawable is the
	 * screen itself, and handing the screen back for an offscreen context
	 * would put two contexts on one framebuffer.
	 *
	 * UNVERIFIED ON HARDWARE. DDK 1.7 predates EGL_KHR_surfaceless_context
	 * by about a year, so if WebKit reports that it cannot create the
	 * sharing context, that extension is the first thing to check
	 * (eglQueryString(dpy, EGL_EXTENSIONS)). The fallback is a 1x1 pbuffer
	 * via WSEGL_CreatePixmapDrawable, which EGL 1.4 does require.
	 */
	return (EGLNativeWindowType)0;
}

static struct wpe_renderer_backend_egl_offscreen_target_interface pvr_offscreen_target = {
	.create            = ot_create,
	.destroy           = ot_destroy,
	.initialize        = ot_initialize,
	.get_native_window = ot_get_native_window,
};

/* ----------------------------------------------------------- renderer host */

static void *rh_create(void)
{
	return NULL;
}

static void rh_destroy(void *data)
{
	(void)data;
}

static int rh_create_client(void *data)
{
	(void)data;
	/*
	 * The Wayland backend returns a socket to its compositor. There is no
	 * compositor here -- the web process draws to the framebuffer directly
	 * -- so there is no fd to hand back.
	 */
	return -1;
}

static struct wpe_renderer_host_interface pvr_renderer_host = {
	.create        = rh_create,
	.destroy       = rh_destroy,
	.create_client = rh_create_client,
};

/* -------------------------------------------------------------- view backend */

struct pvr_view {
	struct wpe_view_backend *wpe;
};

static void *vb_create(void *params, struct wpe_view_backend *wpe)
{
	struct pvr_view *v;
	(void)params;
	v = calloc(1, sizeof(*v));
	if (v)
		v->wpe = wpe;
	return v;
}

static void vb_destroy(void *data)
{
	free(data);
}

/* Panel size. Overridable so this is not pinned to one board. */
static void pvr_panel_size(uint32_t *w, uint32_t *h)
{
	const char *e = getenv("WPE_PVR_SIZE");
	unsigned a, b;

	*w = 720; *h = 480;
	if (e && sscanf(e, "%ux%u", &a, &b) == 2 && a && b) {
		*w = a; *h = b;
	}
}

static void vb_initialize(void *data)
{
	struct pvr_view *v = data;
	uint32_t w, h;

	/*
	 * WPE does not render until the view reports a size, so this is not
	 * optional -- omit it and the first symptom is a hang with nothing in
	 * the log.
	 */
	if (v && v->wpe) {
		pvr_panel_size(&w, &h);
		wpe_view_backend_dispatch_set_size(v->wpe, w, h);
	}
}

static int vb_get_renderer_host_fd(void *data)
{
	(void)data;
	return -1;
}

static struct wpe_view_backend_interface pvr_view_backend = {
	.create              = vb_create,
	.destroy             = vb_destroy,
	.initialize          = vb_initialize,
	.get_renderer_host_fd = vb_get_renderer_host_fd,
};

/* ------------------------------------------------------------------ loader */

__attribute__((visibility("default")))
struct wpe_loader_interface _wpe_loader_interface = {
	.load_object = NULL,   /* filled in below */
};

static void *pvr_load_object(const char *name)
{
	if (!name)
		return NULL;
	if (!strcmp(name, "_wpe_renderer_host_interface"))
		return &pvr_renderer_host;
	if (!strcmp(name, "_wpe_renderer_backend_egl_interface"))
		return &pvr_renderer_backend;
	if (!strcmp(name, "_wpe_renderer_backend_egl_target_interface"))
		return &pvr_renderer_target;
	if (!strcmp(name, "_wpe_renderer_backend_egl_offscreen_target_interface"))
		return &pvr_offscreen_target;
	if (!strcmp(name, "_wpe_view_backend_interface"))
		return &pvr_view_backend;

	/* Say so rather than returning NULL silently: libwpe's failure mode for
	 * an unknown object is a null-deref somewhere else entirely. */
	fprintf(stderr, "wpebackend-pvr: no such object '%s'\n", name);
	return NULL;
}

__attribute__((constructor))
static void pvr_init(void)
{
	_wpe_loader_interface.load_object = pvr_load_object;
}
