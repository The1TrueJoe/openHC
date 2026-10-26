# webview
#
# WPE_PC: WebKit 2.44 installs wpe-webkit-2.0.pc and no wpe-webkit-1.1.pc --
# checked against the generated files in an actual 2.44 build tree, because the
# naming has moved around across releases and guessing it wrong costs a whole
# build. The API this file uses is unchanged between the two: API 2.0 still has
# webkit_web_view_backend_new(struct wpe_view_backend *, GDestroyNotify,
# gpointer) and webkit_web_view_new(WebKitWebViewBackend *). Detect rather than
# hard-code, so an older or newer WebKit still builds.
WPE_PC   ?= $(shell pkg-config --exists wpe-webkit-2.0 \
              && echo wpe-webkit-2.0 || echo wpe-webkit-1.1)
# The display path (see src/webview.c): WPE Platform when WebKit was built with
# it (wpe-platform-2.0.pc exists -- the HC-800, BR2_PACKAGE_WPEWEBKIT_WPE_PLATFORM),
# else the libwpe backend API (the EAs).
PLATFORM_PC ?= $(shell pkg-config --exists wpe-platform-2.0 && echo wpe-platform-2.0)
PREFIX   ?= /usr
CC       ?= cc
# The libwpe path reads /dev/input itself (src/input.c), keymapped by xkbcommon;
# the platform does its own input.
ifneq ($(PLATFORM_PC),)
PKGS      = $(WPE_PC) $(PLATFORM_PC) glib-2.0 gobject-2.0
CPPFLAGS += -DOHC_WPE_PLATFORM
SRCS      = src/webview.c
else
PKGS      = $(WPE_PC) wpe-1.0 glib-2.0 gobject-2.0 xkbcommon
SRCS      = src/webview.c src/input.c
endif
CFLAGS   ?= -O2 -g -Wall -Wextra
LDLIBS   += $(shell pkg-config --libs $(PKGS))
CPPFLAGS += $(shell pkg-config --cflags $(PKGS))

webview: $(SRCS) src/input.h
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ $(SRCS) $(LDFLAGS) $(LDLIBS)

install: webview
	install -D -m 0755 webview $(DESTDIR)$(PREFIX)/bin/webview

clean:
	rm -f webview

.PHONY: install clean
