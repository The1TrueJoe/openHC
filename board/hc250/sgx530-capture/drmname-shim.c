// SPDX-License-Identifier: GPL-2.0
// LD_PRELOAD shim for the capture boot: DRM_IOCTL_VERSION on a device whose driver
// is $DRM_REAL_NAME (default omapdrm) reports $DRM_FAKE_NAME instead, to test
// whether a user-space device scan matches on the driver name. Nothing else changes.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

struct drm_version32 {
	int major, minor, patch;
	size_t name_len;
	char *name;
	size_t date_len;
	char *date;
	size_t desc_len;
	char *desc;
};

#define DRM_IOCTL_VERSION 0xC0246400u

int ioctl(int fd, unsigned long req, ...)
{
	static int (*real)(int, unsigned long, void *);
	va_list ap;
	void *arg;
	int r;

	va_start(ap, req);
	arg = va_arg(ap, void *);
	va_end(ap);
	if (!real)
		real = (int (*)(int, unsigned long, void *))dlsym(RTLD_NEXT, "ioctl");
	r = real(fd, req, arg);
	if (r == 0 && (unsigned)req == DRM_IOCTL_VERSION && arg) {
		struct drm_version32 *v = arg;
		const char *want = getenv("DRM_REAL_NAME") ? getenv("DRM_REAL_NAME") : "omapdrm";
		const char *fake = getenv("DRM_FAKE_NAME");
		char buf[64] = { 0 };
		size_t n;

		if (!fake)
			return r;
		if (!v->name) {		/* the length query: leave room for the fake */
			if (v->name_len < strlen(fake))
				v->name_len = strlen(fake);
			return r;
		}
		n = v->name_len < sizeof(buf) - 1 ? v->name_len : sizeof(buf) - 1;
		memcpy(buf, v->name, n);
		if (strcmp(buf, want) == 0) {
			n = strlen(fake);
			memcpy(v->name, fake, n);
			v->name_len = n;
		}
	}
	return r;
}
