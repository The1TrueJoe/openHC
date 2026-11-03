/* SPDX-License-Identifier: MIT */
#include "ir.h"
#include "test.h"
#include "vectors.h"

#define CLK 50000000u

/* the IROUT_SEND payload inside a captured frame */
static const uint8_t *irsend_payload(void) { return &ea1_irsend[7]; }

static void put16(uint8_t *p, uint16_t v)
{
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)v;
}

static uint16_t build(uint8_t *p, uint8_t mode, uint32_t mask, uint16_t carrier,
                      uint16_t once, uint16_t rep, const uint16_t *w, uint16_t n)
{
    memset(p, 0, OHC_IR_FIXED);
    p[0] = mode;
    p[1] = (uint8_t)(mask >> 16);
    p[2] = (uint8_t)(mask >> 8);
    p[3] = (uint8_t)mask;
    put16(&p[8], carrier);
    put16(&p[10], once);
    put16(&p[12], rep);
    for (uint16_t i = 0; i < n; i++)
        put16(&p[OHC_IR_FIXED + 2 * i], w[i]);
    return (uint16_t)(OHC_IR_FIXED + 2 * n);
}

static void tx_captured(void)
{
    ir_tx_job j;
    ir_tx_cursor c = { 0, 0 };
    bool mark;
    uint32_t t;
    static const uint16_t want[] = { 0x157, 0xac, 0x16, 0x16 };

    CHECK_EQ(ir_tx_parse(irsend_payload(), 22, CLK, &j), IR_OK);
    CHECK_EQ(j.mode, 1);
    CHECK_EQ(j.mask, 1);
    CHECK_EQ(j.carrier_hz, 4145146u / 0x6d);    /* 38028 Hz */
    CHECK_EQ(j.carrier_ticks, 1315);
    CHECK_EQ(j.n_words, 4);
    for (int i = 0; i < 4; i++) {
        CHECK(ir_tx_next(&j, &c, &mark, &t));
        CHECK_EQ(mark, i % 2 == 0);
        CHECK_EQ(t, want[i] * 1315u);
    }
    CHECK(!ir_tx_next(&j, &c, &mark, &t));
}

/* what the kernel driver sends: mode 1, counts 0, an odd word count ending on a mark */
static void tx_driver(void)
{
    uint8_t p[64];
    static const uint16_t w[] = { 342, 171, 22, 64, 22 };
    ir_tx_job j;
    ir_tx_cursor c = { 0, 0 };
    bool mark = false;
    uint32_t t;
    int n = 0;

    CHECK_EQ(ir_tx_parse(p, build(p, 1, 1u << 6, 4145146u / 38000u, 0, 0, w, 5), CLK, &j), IR_OK);
    CHECK_EQ(j.mask, 0x40);
    while (ir_tx_next(&j, &c, &mark, &t))
        n++;
    CHECK_EQ(n, 5);
    CHECK(mark);
    /* a duration of 0x7fff is the longest; bit 15 is not ours to read */
    p[OHC_IR_FIXED] = 0xff;
    p[OHC_IR_FIXED + 1] = 0xff;
    CHECK_EQ(ir_tx_parse(p, OHC_IR_FIXED + 10, CLK, &j), IR_OK);
    c.index = c.pass = 0;
    ir_tx_next(&j, &c, &mark, &t);
    CHECK_EQ(t, 0x7fffu * j.carrier_ticks);
}

static void tx_rejects(void)
{
    uint8_t p[64];
    static const uint16_t w[] = { 10, 10 };
    ir_tx_job j;

    /* the word that hard-faults stock: refused, never divided by */
    CHECK_EQ(ir_tx_parse(p, build(p, 1, 1, 0, 0, 0, w, 2), CLK, &j), IR_ERR_CARRIER);
    CHECK_EQ(ir_tx_parse(p, build(p, 1, 1, 4145146u / 19000u, 0, 0, w, 2), CLK, &j), IR_ERR_CARRIER);
    CHECK_EQ(ir_tx_parse(p, build(p, 1, 1, 8, 0, 0, w, 2), CLK, &j), IR_ERR_CARRIER);
    CHECK_EQ(ir_tx_parse(p, 13, CLK, &j), IR_ERR_SHORT);
    CHECK_EQ(ir_tx_parse(p, 17, CLK, &j), IR_ERR_SHORT);
    CHECK_EQ(ir_tx_parse(p, build(p, 1, 1, 0x6d, 0, 0, w, 0), CLK, &j), IR_ERR_EMPTY);
    /* mode >= 2 reads the pair counts, and they must add up */
    CHECK_EQ(ir_tx_parse(p, build(p, 3, 1, 0x6d, 2, 0, w, 2), CLK, &j), IR_ERR_PAIRS);
}

static void tx_repeats(void)
{
    uint8_t p[64];
    static const uint16_t w[] = { 1, 2, 3, 4 };    /* once: 1 2, repeat: 3 4 */
    static const uint16_t want[] = { 1, 2, 3, 4, 3, 4, 3, 4 };
    ir_tx_job j;
    ir_tx_cursor c = { 0, 0 };
    bool mark;
    uint32_t t;
    int n = 0;

    CHECK_EQ(ir_tx_parse(p, build(p, 3, 1, 0x6d, 1, 1, w, 4), CLK, &j), IR_OK);
    while (ir_tx_next(&j, &c, &mark, &t)) {
        CHECK(n < 8);
        if (n < 8) {
            CHECK_EQ(t / j.carrier_ticks, want[n]);
            CHECK_EQ(mark, n % 2 == 0);
        }
        n++;
    }
    CHECK_EQ(n, 8);   /* three passes: whole, then the repeat sequence twice */

    /* 0xff goes until stopped */
    CHECK_EQ(ir_tx_parse(p, build(p, 0xff, 1, 0x6d, 1, 1, w, 4), CLK, &j), IR_OK);
    c.index = c.pass = 0;
    for (n = 0; n < 1000; n++)
        CHECK(ir_tx_next(&j, &c, &mark, &t));

    /* without a repeat sequence a pass replays everything */
    CHECK_EQ(ir_tx_parse(p, build(p, 2, 1, 0x6d, 2, 0, w, 4), CLK, &j), IR_OK);
    c.index = c.pass = 0;
    for (n = 0; ir_tx_next(&j, &c, &mark, &t); n++)
        ;
    CHECK_EQ(n, 8);
}

/* edges a carrier receiver produces for bursts of `periods` cycles */
static uint32_t play(ir_rx *rx, uint32_t t, const uint16_t *words, int n, uint32_t period)
{
    for (int i = 0; i < n; i++) {
        uint32_t len = words[i] & OHC_IRIN_DURATION;

        if (words[i] & OHC_IRIN_MARK) {
            for (uint32_t k = 0; k < len; k++)
                ir_rx_edge(rx, t + k * period + (k * 7u) % 5u);    /* a little jitter */
        }
        t += len * period;
    }
    return t;
}

static void rx_nec(void)
{
    static uint16_t words[128], code[67];
    uint8_t frame[IR_CAP_FRAME_BYTES];
    ir_rx rx;
    ir_cap cap;
    uint32_t t;
    int i;

    /* NEC: the captured head, then 32 bits and a stop mark */
    memcpy(code, nec_head, sizeof(nec_head));
    for (i = 8; i < 66; i += 2) {
        code[i] = 0x8016;
        code[i + 1] = (i / 2) % 3 ? 0x0016 : 0x003f;
    }
    code[66] = 0x8016;

    ir_rx_init(&rx, words, 128, CLK, false);
    t = play(&rx, 1000, code, 67, nec_carrier);
    CHECK(!ir_rx_poll(&rx, t + 1000));
    CHECK(ir_rx_poll(&rx, t + CLK / 1000000u * IR_RX_IDLE_US + 1));
    CHECK_EQ(ir_rx_carrier(&rx), nec_carrier);
    CHECK_EQ(rx.count, 67);
    for (i = 0; i < 67; i++)
        CHECK_EQ(words[i], code[i]);

    cap.carrier = (uint16_t)ir_rx_carrier(&rx);
    cap.words = words;
    cap.count = rx.count;
    cap.sent = 0;
    CHECK_EQ(ir_cap_next(&cap, frame), 2 + 67 * 2);
    CHECK_EQ(frame[0], 0x05);
    CHECK_EQ(frame[1], 0x15);
    for (i = 0; i < 8; i++)
        CHECK_EQ((frame[2 + 2 * i] << 8) | frame[3 + 2 * i], nec_head[i]);
    CHECK_EQ(ir_cap_next(&cap, frame), 0);

    /* the timestamp wraps mid-code */
    ir_rx_init(&rx, words, 128, CLK, false);
    t = play(&rx, 0xffffffffu - 200000u, code, 67, nec_carrier);
    CHECK(ir_rx_poll(&rx, t + CLK / 1000000u * IR_RX_IDLE_US + 1));
    CHECK_EQ(rx.count, 67);
    CHECK_EQ(words[1], code[1]);
}

static void rx_chunks(void)
{
    static uint16_t words[300];
    uint8_t frame[IR_CAP_FRAME_BYTES];
    ir_cap cap = { 1301, words, 250, 0 };

    for (int i = 0; i < 250; i++)
        words[i] = (uint16_t)i;
    CHECK_EQ(ir_cap_next(&cap, frame), 202);    /* stock's 100-word chunk */
    CHECK_EQ(frame[0], 0x05);
    CHECK_EQ(ir_cap_next(&cap, frame), 202);
    CHECK_EQ(frame[0], 0x05);                   /* every chunk repeats the carrier */
    CHECK_EQ((frame[2] << 8) | frame[3], 100);
    CHECK_EQ(ir_cap_next(&cap, frame), 102);
    CHECK_EQ(ir_cap_next(&cap, frame), 0);
    CHECK_EQ(ir_cap_word(true, 99999), 0x8000 | OHC_IRIN_DURATION);
}

static void rx_envelope(void)
{
    static uint16_t words[16];
    uint32_t p = CLK / IR_RX_NOMINAL_HZ;
    ir_rx rx;

    ir_rx_init(&rx, words, 16, CLK, true);
    ir_rx_level(&rx, 100, false);       /* idle before a code: ignored */
    ir_rx_level(&rx, 1000, true);
    ir_rx_level(&rx, 1000 + 342 * p, false);
    ir_rx_level(&rx, 1000 + 512 * p, true);
    ir_rx_level(&rx, 1000 + 534 * p, false);
    ir_rx_level(&rx, 1000 + 556 * p, true);
    ir_rx_level(&rx, 1000 + 556 * p + 3, true);     /* repeated level: noise */
    ir_rx_level(&rx, 1000 + 578 * p, false);
    CHECK(ir_rx_poll(&rx, 1000 + 578 * p + CLK / 1000000u * IR_RX_IDLE_US + 1));
    CHECK_EQ(rx.count, 5);
    CHECK_EQ(words[0], 0x8000 | 342);
    CHECK_EQ(words[1], 170);
    CHECK_EQ(words[4], 0x8000 | 22);
}

static void rx_noise(void)
{
    static uint16_t words[16];
    ir_rx rx;

    ir_rx_init(&rx, words, 16, CLK, false);
    ir_rx_edge(&rx, 5000);              /* one blip */
    CHECK(!ir_rx_poll(&rx, 5000 + CLK / 1000000u * IR_RX_IDLE_US + 1));
    CHECK_EQ(rx.count, 0);

    /* overflow is flagged, not written past */
    {
        static uint16_t small[4];
        static const uint16_t code[] = { 0x8010, 0x0010, 0x8010, 0x0010, 0x8010, 0x0010, 0x8010 };

        ir_rx_init(&rx, small, 4, CLK, false);
        play(&rx, 0, code, 7, 1316);
        CHECK(rx.overflow);
        CHECK_EQ(rx.count, 4);
    }
}

void test_ir(void)
{
    tx_captured();
    tx_driver();
    tx_rejects();
    tx_repeats();
    rx_nec();
    rx_chunks();
    rx_envelope();
    rx_noise();
}
