/* SPDX-License-Identifier: MIT */
#include "proto.h"

uint8_t ohc_checksum(const uint8_t *body, size_t len)
{
    uint8_t sum = 0;

    for (size_t i = 0; i < len; i++)
        sum = (uint8_t)(sum + body[i]);
    return (uint8_t)(0u - sum);
}

void ohc_rx_init(ohc_rx *rx, ohc_frame_cb cb, void *user)
{
    rx->state = OHC_RX_IDLE;
    rx->body_len = 0;
    rx->need = 0;
    rx->cb = cb;
    rx->user = user;
    rx->bad_checksums = 0;
    rx->resyncs = 0;
}

static void resync(ohc_rx *rx)
{
    rx->state = OHC_RX_IDLE;
    rx->body_len = 0;
    rx->resyncs++;
}

static void deliver(ohc_rx *rx)
{
    const uint8_t *b = rx->body;
    uint16_t n = (uint16_t)(rx->need - 1u);
    ohc_frame f;

    if (ohc_checksum(b, n) != b[n]) {
        rx->bad_checksums++;
        return;
    }
    f.opcode = b[0];
    f.seq = b[1];
    f.flags = b[2];
    f.length = (uint16_t)((b[3] << 8) | b[4]);
    f.payload = &b[OHC_HDR_LEN];
    if (rx->cb)
        rx->cb(&f, rx->user);
}

static void push(ohc_rx *rx, uint8_t byte)
{
    rx->body[rx->body_len++] = byte;
    if (rx->body_len == OHC_HDR_LEN) {
        uint16_t len = (uint16_t)((rx->body[3] << 8) | rx->body[4]);

        if (len > OHC_MAX_PAYLOAD) {
            resync(rx);
            return;
        }
        rx->need = (uint16_t)(OHC_HDR_LEN + len + 1u);
    }
    if (rx->body_len > OHC_HDR_LEN - 1u && rx->body_len == rx->need) {
        rx->state = OHC_RX_IDLE;
        rx->body_len = 0;
        deliver(rx);
    }
}

void ohc_rx_feed(ohc_rx *rx, const uint8_t *data, size_t len)
{
    for (size_t i = 0; i < len; i++) {
        uint8_t b = data[i];

        switch (rx->state) {
        case OHC_RX_IDLE:
            if (b == OHC_DLE)
                rx->state = OHC_RX_STX;
            break;
        case OHC_RX_STX:
            if (b == OHC_STX) {
                rx->state = OHC_RX_BODY;
                rx->body_len = 0;
                rx->need = 0;
            } else if (b != OHC_DLE) {
                rx->state = OHC_RX_IDLE;
            }
            break;
        case OHC_RX_BODY:
            if (b == OHC_DLE)
                rx->state = OHC_RX_BODY_DLE;
            else
                push(rx, b);
            break;
        case OHC_RX_BODY_DLE:
            if (b == OHC_DLE) {
                rx->state = OHC_RX_BODY;
                push(rx, OHC_DLE);
            } else if (b == OHC_STX) {
                /* unstuffed DLE STX: the last frame was cut short */
                rx->state = OHC_RX_BODY;
                rx->body_len = 0;
                rx->need = 0;
                rx->resyncs++;
            } else {
                resync(rx);
            }
            break;
        }
    }
}

static void put_stuffed(uint8_t b, ohc_put_fn put, void *user)
{
    if (b == OHC_DLE)
        put(OHC_DLE, user);
    put(b, user);
}

void ohc_emit(uint8_t opcode, uint8_t seq, uint8_t flags,
              const uint8_t *payload, uint16_t length, ohc_put_fn put, void *user)
{
    uint8_t hdr[OHC_HDR_LEN] = {
        opcode, seq, flags, (uint8_t)(length >> 8), (uint8_t)length,
    };
    uint8_t sum = 0;

    put(OHC_DLE, user);
    put(OHC_STX, user);
    for (unsigned i = 0; i < OHC_HDR_LEN; i++) {
        sum = (uint8_t)(sum + hdr[i]);
        put_stuffed(hdr[i], put, user);
    }
    for (uint16_t i = 0; i < length; i++) {
        sum = (uint8_t)(sum + payload[i]);
        put_stuffed(payload[i], put, user);
    }
    put_stuffed((uint8_t)(0u - sum), put, user);
}

struct buf {
    uint8_t *p;
    size_t n, cap;
};

static void buf_put(uint8_t b, void *user)
{
    struct buf *bf = user;

    if (bf->n < bf->cap)
        bf->p[bf->n] = b;
    bf->n++;
}

size_t ohc_encode(uint8_t opcode, uint8_t seq, uint8_t flags,
                  const uint8_t *payload, uint16_t length, uint8_t *out, size_t cap)
{
    struct buf bf = { out, 0, cap };

    if (length > OHC_MAX_PAYLOAD)
        return 0;
    ohc_emit(opcode, seq, flags, payload, length, buf_put, &bf);
    return bf.n <= cap ? bf.n : 0;
}
