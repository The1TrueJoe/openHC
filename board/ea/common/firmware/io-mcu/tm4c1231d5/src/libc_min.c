/* The handful of libc functions gcc emits calls to, provided locally.
 *
 * The firmware links -nostdlib: no newlib, no heap, nothing we did not write.
 * gcc still lowers struct copies and array init to memcpy/memset regardless of
 * -ffreestanding, so these must exist or the link fails.
 */
#include <stddef.h>

void *memcpy(void *dst, const void *src, size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n--) {
        *d++ = *s++;
    }
    return dst;
}

void *memset(void *dst, int c, size_t n)
{
    unsigned char *d = dst;
    while (n--) {
        *d++ = (unsigned char)c;
    }
    return dst;
}

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p) {
        p++;
    }
    return (size_t)(p - s);
}
