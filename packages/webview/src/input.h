/* USB keyboards and mice for the libwpe display path; see input.c. */
#ifndef WEBVIEW_INPUT_H
#define WEBVIEW_INPUT_H

#include <wpe/wpe.h>

/* Open every keyboard and pointer under /dev/input, now and as they are
 * plugged in, and feed them to this view backend on the default main context. */
void input_start(struct wpe_view_backend *backend);

#endif
