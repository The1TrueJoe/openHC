# wpebackend-pvr
#
# A plain Makefile rather than meson: this builds one shared object from one
# source file, and the only thing it needs to find is libwpe's headers.
#
#   make                       # needs libwpe via pkg-config
#   make WPE_CFLAGS=-I/wpe EGL_CFLAGS=-I/ddk   # or point at headers directly
#
# EGL headers are needed even to compile the scaffold: libwpe's own
# renderer-backend-egl.h includes <EGL/eglplatform.h>. Point EGL_CFLAGS at the
# DDK's headers.
#
# MESA_EGL_NO_X11_HEADERS matters: the DDK's eglplatform.h defaults to X11 and
# includes Xlib.h. That macro switches EGLNativeWindowType to a plain integer,
# which is both what the LinuxFB WSEGL wants and what lets this build on a
# system with no X headers at all.

PREFIX      ?= /usr
LIBDIR      ?= $(PREFIX)/lib
CC          ?= cc
WPE_CFLAGS  ?= $(shell pkg-config --cflags wpe-1.0 2>/dev/null)
WPE_LIBS    ?= $(shell pkg-config --libs   wpe-1.0 2>/dev/null)
EGL_CFLAGS  ?= $(shell pkg-config --cflags egl 2>/dev/null)

# -fvisibility=hidden: libwpe looks up exactly one symbol,
# _wpe_loader_interface, which is marked visible in the source. Everything else
# staying private keeps the backend from colliding with the DDK's own symbols,
# which are numerous and not namespaced.
CFLAGS  ?= -O2 -g -Wall -Wextra -fPIC -fvisibility=hidden
LDFLAGS ?= -shared

SONAME  = libWPEBackend-pvr.so.1
TARGET  = $(SONAME).0.0
SRC     = src/pvr-backend.c

all: $(TARGET)

$(TARGET): $(SRC)
	$(CC) $(CFLAGS) -DMESA_EGL_NO_X11_HEADERS $(WPE_CFLAGS) $(EGL_CFLAGS) $(LDFLAGS) -Wl,-soname,$(SONAME) \
		-o $@ $(SRC) $(WPE_LIBS)
	ln -sf $(TARGET) $(SONAME)
	ln -sf $(SONAME) libWPEBackend-pvr.so

install: $(TARGET)
	install -D -m 0755 $(TARGET) $(DESTDIR)$(LIBDIR)/$(TARGET)
	ln -sf $(TARGET) $(DESTDIR)$(LIBDIR)/$(SONAME)
	ln -sf $(SONAME) $(DESTDIR)$(LIBDIR)/libWPEBackend-pvr.so

clean:
	rm -f $(TARGET) $(SONAME) libWPEBackend-pvr.so

.PHONY: all install clean
