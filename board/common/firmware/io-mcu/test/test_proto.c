/* SPDX-License-Identifier: MIT */
#include "test.h"
#include "vectors.h"

static ohc_frame got[8];
static uint8_t got_payload[8][OHC_MAX_PAYLOAD];
static int n_got;

static void keep(const ohc_frame *f, void *user)
{
    (void)user;
    if (n_got >= 8)
        return;
    memcpy(got_payload[n_got], f->payload, f->length);
    got[n_got] = *f;
    got[n_got].payload = got_payload[n_got];
    n_got++;
}

static ohc_rx rx;

static int decode(const uint8_t *b, size_t n, size_t chunk)
{
    n_got = 0;
    ohc_rx_init(&rx, keep, 0);
    for (size_t i = 0; i < n; i += chunk)
        ohc_rx_feed(&rx, b + i, i + chunk <= n ? chunk : n - i);
    return n_got;
}

/* decode, then encode the result back: both must reproduce the capture */
static void roundtrip(const uint8_t *b, size_t n)
{
    uint8_t out[64];
    size_t m;

    for (size_t chunk = 1; chunk <= n; chunk++) {
        CHECK_EQ(decode(b, n, chunk), 1);
        CHECK_EQ(rx.bad_checksums, 0);
    }
    decode(b, n, n);
    CHECK_EQ(got[0].opcode, b[2]);
    CHECK_EQ(got[0].seq, b[3]);
    CHECK_EQ(got[0].flags, b[4]);
    CHECK_EQ(got[0].length, (b[5] << 8) | b[6]);
    m = ohc_encode(got[0].opcode, got[0].seq, got[0].flags, got[0].payload,
                   got[0].length, out, sizeof(out));
    CHECK_EQ(m, n);
    CHECK_MEM(out, b, n);
}

static void captured_frames(void)
{
    roundtrip(V(hc_fwv_req));
    roundtrip(V(hc_fwv_resp));
    roundtrip(V(hc_pn_req));
    roundtrip(V(hc_pn_resp));
    roundtrip(V(hc_ab_req));
    roundtrip(V(hc_ab_resp));
    roundtrip(V(ea_pn_req));
    roundtrip(V(ea_ab_resp));
    roundtrip(V(ea_census0));
    roundtrip(V(ea_census1));
    roundtrip(V(ea1_irsend));
    roundtrip(V(ea1_irstatus));

    decode(V(hc_fwv_resp), 3);
    CHECK_MEM(got[0].payload, "03.26.15", 8);
}

static void stuffing(void)
{
    /* every byte a DLE, and a checksum that comes out as DLE */
    uint8_t p[6] = { 0x10, 0x10, 0x10, 0x00, 0x10, 0x10 };
    uint8_t out[64];
    size_t n;
    uint8_t ck;

    for (unsigned v = 0; v < 256; v++) {
        p[3] = (uint8_t)v;
        n = ohc_encode(0x10, 0x10, 0x10, p, sizeof(p), out, sizeof(out));
        CHECK(n > 0);
        CHECK_EQ(decode(out, n, 1), 1);
        CHECK_EQ(got[0].opcode, 0x10);
        CHECK_EQ(got[0].seq, 0x10);
        CHECK_EQ(got[0].length, sizeof(p));
        CHECK_MEM(got[0].payload, p, sizeof(p));
    }
    /* find a payload whose checksum is DLE and make sure it is doubled */
    for (unsigned v = 0; v < 256; v++) {
        uint8_t body[6] = { 0x20, 0, 0, 0, 1, (uint8_t)v };

        ck = ohc_checksum(body, 6);
        if (ck != OHC_DLE)
            continue;
        n = ohc_encode(0x20, 0, 0, &body[5], 1, out, sizeof(out));
        CHECK_EQ(out[n - 1], OHC_DLE);
        CHECK_EQ(out[n - 2], OHC_DLE);
        CHECK_EQ(decode(out, n, 1), 1);
    }
    /* too small a buffer is refused, not truncated */
    CHECK_EQ(ohc_encode(1, 2, 3, p, sizeof(p), out, 10), 0);
}

static void noise(void)
{
    uint8_t b[128];
    size_t n = 0;

    /* line noise, a stray DLE, DLE DLE, then a frame */
    b[n++] = 0xff; b[n++] = 0x55; b[n++] = 0x10; b[n++] = 0x41;
    b[n++] = 0x10; b[n++] = 0x10;
    memcpy(b + n, hc_fwv_req, sizeof(hc_fwv_req));
    n += sizeof(hc_fwv_req);
    CHECK_EQ(decode(b, n, 1), 1);
    CHECK_EQ(got[0].opcode, 0x34);

    /* bad checksum: dropped and counted */
    memcpy(b, hc_fwv_resp, sizeof(hc_fwv_resp));
    b[sizeof(hc_fwv_resp) - 1] ^= 1;
    CHECK_EQ(decode(b, sizeof(hc_fwv_resp), 1), 0);
    CHECK_EQ(rx.bad_checksums, 1);

    /* truncated, then a new frame: the second survives */
    memcpy(b, hc_pn_resp, 12);
    memcpy(b + 12, hc_ab_req, sizeof(hc_ab_req));
    CHECK_EQ(decode(b, 12 + sizeof(hc_ab_req), 1), 1);
    CHECK_EQ(got[0].opcode, 0xd2);
    CHECK(rx.resyncs >= 1);

    /* a length over the maximum resynchronises instead of swallowing the stream */
    n = 0;
    b[n++] = 0x10; b[n++] = 0x02; b[n++] = 0x24; b[n++] = 0; b[n++] = 0;
    b[n++] = 0xff; b[n++] = 0xff;
    memcpy(b + n, ea_pn_req, sizeof(ea_pn_req));
    n += sizeof(ea_pn_req);
    CHECK_EQ(decode(b, n, 1), 1);
    CHECK_EQ(got[0].opcode, 0x24);

    /* two frames in one read */
    memcpy(b, ea_census0, sizeof(ea_census0));
    memcpy(b + sizeof(ea_census0), ea_census1, sizeof(ea_census1));
    CHECK_EQ(decode(b, sizeof(ea_census0) + sizeof(ea_census1), 64), 2);
    CHECK_EQ(got[1].payload[0], 1);
}

static void largest(void)
{
    static uint8_t p[OHC_MAX_PAYLOAD], out[2 * OHC_MAX_PAYLOAD + 16];
    size_t n;

    for (unsigned i = 0; i < sizeof(p); i++)
        p[i] = (uint8_t)(i * 7);
    n = ohc_encode(0x66, 9, 0, p, sizeof(p), out, sizeof(out));
    CHECK(n > sizeof(p));
    CHECK_EQ(decode(out, n, 7), 1);
    CHECK_EQ(got[0].length, OHC_MAX_PAYLOAD);
    CHECK_MEM(got[0].payload, p, sizeof(p));
    CHECK_EQ(ohc_encode(0x66, 9, 0, p, OHC_MAX_PAYLOAD + 1, out, sizeof(out)), 0);
}

void test_proto(void)
{
    captured_frames();
    stuffing();
    noise();
    largest();
}
