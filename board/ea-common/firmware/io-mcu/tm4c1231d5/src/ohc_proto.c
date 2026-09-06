#include "ohc_proto.h"

#include <string.h>

uint8_t ohc_checksum(const uint8_t *body, size_t len)
{
    uint8_t sum = 0;
    for (size_t i = 0; i < len; i++) {
        sum = (uint8_t)(sum + body[i]);
    }
    return (uint8_t)(-(int)sum); /* negated 8-bit sum */
}

void ohc_rx_init(ohc_rx *rx, ohc_frame_cb cb, void *user)
{
    memset(rx, 0, sizeof(*rx));
    rx->cb = cb;
    rx->user = user;
}

/* Body layout once un-stuffed: opcode, seq, flags, len_hi, len_lo, payload...,
 * checksum. `need` is only known after the 5th byte. */
#define HEADER_LEN 5u

static void deliver(ohc_rx *rx)
{
    const uint8_t *b = rx->body;
    uint16_t body_no_cksum = (uint16_t)(rx->need - 1u);
    uint8_t want = ohc_checksum(b, body_no_cksum);

    if (want != b[body_no_cksum]) {
        rx->bad_checksums++;
        return; /* drop it; the sender will retry or the stream resyncs */
    }
    if (rx->cb) {
        ohc_frame f;
        f.opcode = b[0];
        f.seq    = b[1];
        f.flags  = b[2];
        f.length = (uint16_t)((b[3] << 8) | b[4]);
        if (f.length > OHC_MAX_PAYLOAD) {
            return;
        }
        memcpy(f.payload, &b[HEADER_LEN], f.length);
        rx->cb(&f, rx->user);
    }
}

static void push_body_byte(ohc_rx *rx, uint8_t byte)
{
    if (rx->body_len >= sizeof(rx->body)) {
        /* Oversized: abandon and hunt for the next frame rather than smearing
         * the overflow into the following one. */
        rx->state = OHC_RX_IDLE;
        rx->body_len = 0;
        rx->resyncs++;
        return;
    }
    rx->body[rx->body_len++] = byte;

    if (rx->body_len == HEADER_LEN) {
        uint16_t length = (uint16_t)((rx->body[3] << 8) | rx->body[4]);
        if (length > OHC_MAX_PAYLOAD) {
            rx->state = OHC_RX_IDLE;
            rx->body_len = 0;
            rx->resyncs++;
            return;
        }
        rx->need = (uint16_t)(HEADER_LEN + length + 1u); /* + checksum */
    }
    if (rx->body_len >= HEADER_LEN && rx->body_len == rx->need) {
        deliver(rx);
        rx->state = OHC_RX_IDLE;
        rx->body_len = 0;
    }
}

void ohc_rx_feed(ohc_rx *rx, const uint8_t *data, size_t len)
{
    for (size_t i = 0; i < len; i++) {
        uint8_t byte = data[i];
        switch (rx->state) {
        case OHC_RX_IDLE:
            if (byte == OHC_DLE) {
                rx->state = OHC_RX_STX;
            }
            break;

        case OHC_RX_STX:
            if (byte == OHC_STX) {
                rx->state = OHC_RX_BODY;
                rx->body_len = 0;
                rx->need = 0;
            } else if (byte == OHC_DLE) {
                /* DLE DLE outside a frame: stay armed, this may be the real
                 * start. Cheap, and it stops a doubled DLE from eating the
                 * following STX. */
            } else {
                rx->state = OHC_RX_IDLE;
            }
            break;

        case OHC_RX_BODY:
            if (byte == OHC_DLE) {
                rx->state = OHC_RX_BODY_DLE;
            } else {
                push_body_byte(rx, byte);
            }
            break;

        case OHC_RX_BODY_DLE:
            rx->state = OHC_RX_BODY;
            if (byte == OHC_DLE) {
                push_body_byte(rx, OHC_DLE); /* stuffed literal */
            } else if (byte == OHC_STX) {
                /* An unstuffed DLE STX inside a body means the previous frame
                 * was truncated and a new one has started. Restart cleanly
                 * instead of corrupting both. */
                rx->body_len = 0;
                rx->need = 0;
                rx->resyncs++;
            } else {
                rx->state = OHC_RX_IDLE;
                rx->body_len = 0;
                rx->resyncs++;
            }
            break;
        }
    }
}

static bool put(uint8_t *out, size_t cap, size_t *n, uint8_t byte)
{
    if (*n >= cap) {
        return false;
    }
    out[(*n)++] = byte;
    return true;
}

static bool put_stuffed(uint8_t *out, size_t cap, size_t *n, uint8_t byte)
{
    if (byte == OHC_DLE && !put(out, cap, n, OHC_DLE)) {
        return false;
    }
    return put(out, cap, n, byte);
}

size_t ohc_encode(uint8_t opcode, uint8_t seq, uint8_t flags,
                  const uint8_t *payload, uint16_t length,
                  uint8_t *out, size_t out_cap)
{
    if (length > OHC_MAX_PAYLOAD) {
        return 0;
    }
    uint8_t header[HEADER_LEN] = {
        opcode, seq, flags, (uint8_t)(length >> 8), (uint8_t)length,
    };
    uint8_t sum = 0;
    for (size_t i = 0; i < HEADER_LEN; i++) {
        sum = (uint8_t)(sum + header[i]);
    }
    for (uint16_t i = 0; i < length; i++) {
        sum = (uint8_t)(sum + payload[i]);
    }
    uint8_t cksum = (uint8_t)(-(int)sum);

    size_t n = 0;
    /* The leading DLE STX is a framing marker and is NOT stuffed. */
    if (!put(out, out_cap, &n, OHC_DLE) || !put(out, out_cap, &n, OHC_STX)) {
        return 0;
    }
    for (size_t i = 0; i < HEADER_LEN; i++) {
        if (!put_stuffed(out, out_cap, &n, header[i])) {
            return 0;
        }
    }
    for (uint16_t i = 0; i < length; i++) {
        if (!put_stuffed(out, out_cap, &n, payload[i])) {
            return 0;
        }
    }
    if (!put_stuffed(out, out_cap, &n, cksum)) {
        return 0;
    }
    return n;
}

size_t ohc_encode_reply(const ohc_frame *req, const uint8_t *payload,
                        uint16_t length, uint8_t *out, size_t out_cap)
{
    return ohc_encode((uint8_t)(req->opcode + 1u), req->seq,
                      OHC_FLAG_RESPONSE, payload, length, out, out_cap);
}
