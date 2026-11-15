# webview
#
# Built against WebKit's WPE Platform API (wpe-platform-2.0, from
# BR2_PACKAGE_WPEWEBKIT_WPE_PLATFORM); the display, input and GPU are its DRM
# platform's.
WPE_PC   ?= wpe-webkit-2.0
PREFIX   ?= /usr
CC       ?= cc
PKGS      = $(WPE_PC) wpe-platform-2.0 glib-2.0 gobject-2.0
SRCS      = src/webview.c
CFLAGS   ?= -O2 -g -Wall -Wextra
LDLIBS   += $(shell pkg-config --libs $(PKGS))
CPPFLAGS += $(shell pkg-config --cflags $(PKGS))

webview: $(SRCS)
	$(CC) $(CFLAGS) $(CPPFLAGS) -o $@ $(SRCS) $(LDFLAGS) $(LDLIBS)

install: webview
	install -D -m 0755 webview $(DESTDIR)$(PREFIX)/bin/webview

clean:
	rm -f webview

.PHONY: install clean
