/* SPDX-License-Identifier: MIT */
/* Single-producer single-consumer byte ring; size a power of two, at most 32 K. */
#ifndef OHC_RING_H
#define OHC_RING_H

#include <stdbool.h>
#include <stdint.h>

typedef struct {
    uint8_t *buf;
    uint16_t mask;
    volatile uint16_t head;     /* producer */
    volatile uint16_t tail;     /* consumer */
} ring;

static inline void ring_init(ring *r, uint8_t *buf, uint16_t size)
{
    r->buf = buf;
    r->mask = (uint16_t)(size - 1u);
    r->head = r->tail = 0;
}

static inline uint16_t ring_count(const ring *r)
{
    return (uint16_t)((r->head - r->tail) & 0xffffu);
}

static inline bool ring_put(ring *r, uint8_t c)
{
    if (ring_count(r) > r->mask)
        return false;
    r->buf[r->head & r->mask] = c;
    r->head = (uint16_t)(r->head + 1u);
    return true;
}

static inline int ring_get(ring *r)
{
    int c;

    if (r->head == r->tail)
        return -1;
    c = r->buf[r->tail & r->mask];
    r->tail = (uint16_t)(r->tail + 1u);
    return c;
}

static inline int ring_peek(const ring *r)
{
    return r->head == r->tail ? -1 : r->buf[r->tail & r->mask];
}

#endif
