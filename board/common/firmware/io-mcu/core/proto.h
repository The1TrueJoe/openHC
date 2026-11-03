/* SPDX-License-Identifier: MIT */
/* DLE/STX frame codec. Hardware-free; constants in ohc-iomcu-proto.h. */
#ifndef OHC_PROTO_H
#define OHC_PROTO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "ohc-iomcu-proto.h"

typedef struct {
    uint8_t opcode;
    uint8_t seq;
    uint8_t flags;
    uint16_t length;
    const uint8_t *payload;     /* valid only inside the callback */
} ohc_frame;

typedef void (*ohc_frame_cb)(const ohc_frame *frame, void *user);

typedef enum {
    OHC_RX_IDLE = 0,    /* hunting for DLE */
    OHC_RX_STX,         /* saw DLE */
    OHC_RX_BODY,
    OHC_RX_BODY_DLE,    /* saw DLE in the body: stuffed, or a new frame */
} ohc_rx_state;

typedef struct {
    ohc_rx_state state;
    uint8_t body[OHC_HDR_LEN + OHC_MAX_PAYLOAD + 1];
    uint16_t body_len;
    uint16_t need;      /* whole body incl. checksum, once the header is in */
    ohc_frame_cb cb;
    void *user;
    uint32_t bad_checksums;
    uint32_t resyncs;
} ohc_rx;

void ohc_rx_init(ohc_rx *rx, ohc_frame_cb cb, void *user);
void ohc_rx_feed(ohc_rx *rx, const uint8_t *data, size_t len);
static inline bool ohc_rx_idle(const ohc_rx *rx) { return rx->state == OHC_RX_IDLE; }

uint8_t ohc_checksum(const uint8_t *body, size_t len);

/* Encoder: streams a stuffed frame through put(). */
typedef void (*ohc_put_fn)(uint8_t byte, void *user);
void ohc_emit(uint8_t opcode, uint8_t seq, uint8_t flags,
              const uint8_t *payload, uint16_t length, ohc_put_fn put, void *user);

/* Same, into a buffer. Returns bytes written, or 0 if it does not fit. */
size_t ohc_encode(uint8_t opcode, uint8_t seq, uint8_t flags,
                  const uint8_t *payload, uint16_t length, uint8_t *out, size_t cap);

#endif
