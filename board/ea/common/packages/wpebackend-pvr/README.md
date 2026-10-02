# wpebackend-pvr

A [libwpe](https://github.com/WebPlatformForEmbedded/libwpe) backend that renders
through PowerVR SGX's framebuffer EGL, for systems with the Imagination DDK and
no Wayland.

## Why this exists

WPE WebKit talks to a display system through a `libwpe` backend, loaded at
runtime. The standard one, `wpebackend-fdo`, is built on Wayland — it needs
`EGL_WL_bind_wayland_display` and GBM.

The Imagination DDK 1.7 that ships for PowerVR SGX 5-series predates Wayland
entirely. Its window-system layer (WSEGL) offers X11/DRI, LinuxFB, Front, Blit
and Flip backends, and no Wayland at all, so `wpebackend-fdo` cannot be built
against it, let alone run. On a headless appliance with a framebuffer and no X
server, that leaves WPE with no way to reach the screen.

This backend fills that gap: it drives EGL directly on the DDK's LinuxFB WSEGL,
which is the path such a device already uses for GLES.

## Target and status

Developed against a PowerVR SGX545 (Intel Atom CE5310) on Linux 7.1, where the
DDK reports:

    GL_RENDERER : PowerVR SGX 545
    GL_VERSION  : OpenGL ES 2.0 build 1.7@862890

**Status: implemented, untested.** All three interfaces are filled in and it
builds; it has never been loaded by WPE, and nothing here has rendered a web
page.

It is small because fbdev makes it small, not because parts are missing:

* `get_native_display` returns `EGL_DEFAULT_DISPLAY` — the LinuxFB WSEGL opens
  `/dev/fb0` itself and takes no display handle.
* `get_native_window` returns `0` — there is one surface and it is the screen;
  the WSEGL ignores the value.
* `frame_rendered` acknowledges immediately. On Wayland a backend waits for the
  compositor to release the buffer; here `eglSwapBuffers` blits straight to the
  framebuffer and returns, so the pixels are already on the panel.
* The renderer host has no client fd — there is no compositor to connect to.

The one genuinely unimplemented piece is the offscreen target, which WPE uses
for out-of-window rendering.

Building needs libwpe's headers *and* EGL headers — libwpe's own
`renderer-backend-egl.h` includes `<EGL/eglplatform.h>`, so EGL is required even
for a scaffold. Build with `-DMESA_EGL_NO_X11_HEADERS`: the DDK's
`eglplatform.h` otherwise pulls in Xlib, and that macro also gives the plain
integer `EGLNativeWindowType` the LinuxFB WSEGL expects.

```sh
make WPE_CFLAGS=-I/path/to/libwpe/include EGL_CFLAGS=-I/path/to/ddk/include
```

## What the hardware can do

Measured on that part at 720x480, full-screen layers, via a purpose-built
benchmark:

| layers | opaque | blended | textured |
|---|---|---|---|
| 1 | 30.3 fps | 28.8 | 28.8 |
| 8 | 28.8 fps | 28.8 | 28.8 |

Flat from one layer to eight, so fill rate is not the constraint — the GPU is
comfortably over-provisioned for a launcher-style UI. The ~29 fps ceiling is a
fixed per-frame cost, and it points at the LinuxFB WSEGL blitting the entire
surface on every swap rather than at rendering. `eglSwapInterval(0)` is accepted
and changes nothing, so it is not vsync.

That shapes this backend's design: **the interesting work is in presentation,
not in drawing.** If a swap can be made to flip rather than copy — the DDK also
ships `libpvrPVR2D_FLIPWSEGL.so` — the ceiling should move.

## Licence

MIT. The DDK blobs it runs against are Intel/Imagination proprietary and are not
included.
