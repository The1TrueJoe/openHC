/* SPDX-License-Identifier: MIT */
#include "test.h"

#define A 0
#define B 1
#define C 2
#define D 3
#define E 4
#define F 5
#define G 6
#define H 7

static uint8_t frame[OHC_CFG_HDR + OHC_CFG_MAX_ENTRIES * OHC_CFG_ENTRY + 8];
static uint16_t flen;
static cfg_table t;

static void start(void)
{
    frame[0] = OHC_CFG_VERSION;
    frame[1] = 0;
    flen = OHC_CFG_HDR;
}

static void add(uint8_t kind, uint8_t index, uint8_t port, uint8_t pin, uint8_t flags)
{
    uint8_t *e = &frame[flen];

    e[0] = kind;
    e[1] = index;
    e[2] = port;
    e[3] = pin;
    e[4] = flags;
    frame[1]++;
    flen += OHC_CFG_ENTRY;
}

/* The EA3's IO as decoded from the stock image (shared/io-mcu-pinmap). */
void cfg_ea3(void);
void cfg_ea3(void)
{
    start();
    add(OHC_CFG_IR_OUT, 0, B, 6, 0);
    add(OHC_CFG_IR_OUT, 1, F, 2, 0);
    add(OHC_CFG_IR_OUT, 2, B, 0, 0);
    add(OHC_CFG_IR_OUT, 3, B, 2, 0);
    add(OHC_CFG_IR_OUT, 4, D, 0, 0);
    add(OHC_CFG_IR_OUT, 5, D, 2, 0);
    add(OHC_CFG_IR_OUT, 6, D, 4, 0);        /* front blaster */
    add(OHC_CFG_IR_IN, 0, D, 6, 0);
    add(OHC_CFG_RELAY, 0, A, 2, 0);
    add(OHC_CFG_CONTACT, 0, F, 0, OHC_CFG_F_PULL_UP | OHC_CFG_F_ACTIVE_LOW);
    add(OHC_CFG_UART_RX, 0, E, 4, 0);
    add(OHC_CFG_UART_TX, 0, E, 5, 0);
    add(OHC_CFG_UART_MUX_A, 0, E, 2, 0);
    add(OHC_CFG_UART_MUX_B, 0, B, 1, 0);
    add(OHC_CFG_UART_RX, 1, E, 0, 0);
    add(OHC_CFG_UART_TX, 1, E, 1, 0);
    add(OHC_CFG_UART_MUX_A, 1, E, 3, 0);
    add(OHC_CFG_UART_MUX_B, 1, B, 3, 0);
    add(OHC_CFG_UART_RX, 2, C, 4, 0);
    add(OHC_CFG_UART_TX, 2, C, 5, 0);
    add(OHC_CFG_UART_MUX_A, 2, A, 7, 0);
    add(OHC_CFG_UART_MUX_B, 2, B, 5, 0);
    add(OHC_CFG_HEARTBEAT, 0, C, 7, 0);
}

/* The HC-800's: two PWM carriers gated onto each jack by active-low lines. */
void cfg_hc800(void);
void cfg_hc800(void)
{
    static const uint8_t gate[7][6] = {     /* A port/pin, B port/pin, C port/pin */
        { C, 7, C, 6, F, 4 }, { C, 5, C, 4, F, 5 }, { B, 4, B, 5, A, 7 },
        { B, 6, F, 2, A, 6 }, { E, 0, E, 1, D, 6 }, { E, 2, E, 3, D, 7 },
        { F, 0, D, 5, 0xff, 0 },
    };
    const uint8_t lo = OHC_CFG_F_ACTIVE_LOW;

    start();
    add(OHC_CFG_IR_CARRIER, 0, D, 1, 0);    /* PWM1 */
    add(OHC_CFG_IR_CARRIER, 1, F, 3, 0);    /* PWM5 */
    for (uint8_t j = 0; j < 7; j++) {
        add(OHC_CFG_IR_GATE, j, gate[j][0], gate[j][1], lo | OHC_CFG_F_CARRIER(0));
        add(OHC_CFG_IR_GATE, j, gate[j][2], gate[j][3], lo | OHC_CFG_F_CARRIER(1));
        if (gate[j][4] != 0xff)
            add(OHC_CFG_HOLD, j, gate[j][4], gate[j][5], 0);
    }
    add(OHC_CFG_IR_IN, 0, D, 4, 0);
    add(OHC_CFG_RELAY, 0, A, 5, 0);
    add(OHC_CFG_RELAY, 1, A, 4, 0);
    add(OHC_CFG_RELAY, 2, A, 3, 0);
    add(OHC_CFG_RELAY, 3, A, 2, 0);
    for (uint8_t i = 0; i < 4; i++)
        add(OHC_CFG_CONTACT, i, B, i, OHC_CFG_F_ACTIVE_LOW);
    add(OHC_CFG_HEARTBEAT, 0, F, 1, 0);
}

const uint8_t *cfg_frame(uint16_t *len);
const uint8_t *cfg_frame(uint16_t *len)
{
    *len = flen;
    return frame;
}

static uint8_t one(const part_desc *p, uint8_t kind, uint8_t index, uint8_t port,
                   uint8_t pin, uint8_t flags)
{
    start();
    add(kind, index, port, pin, flags);
    CHECK_EQ(cfg_parse(p, frame, flen, &t), OHC_CFG_R_OK);
    return t.e[0].status;
}

static void tm4c_ea3(void)
{
    uint8_t out[3 + OHC_CFG_MAX_ENTRIES];
    uint16_t n;

    cfg_ea3();
    CHECK_EQ(cfg_parse(&part_tm4c1231d5, frame, flen, &t), OHC_CFG_R_OK);
    CHECK_EQ(t.count, 23);
    for (int i = 0; i < t.count; i++)
        CHECK_EQ(t.e[i].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[0].func->unit, 0);     /* T0CCP0 */
    CHECK_EQ(t.e[6].func->unit, 20);    /* blaster on WT4CCP0 */
    CHECK_EQ(t.e[7].func->unit, 22);    /* receiver on WT5CCP0 */
    CHECK_EQ(t.e[10].func->unit, 5);    /* UART5 */
    CHECK_EQ(t.e[14].func->unit, 7);    /* UART7 */
    CHECK_EQ(t.e[18].func->unit, 4);    /* PC4/PC5: UART4 */
    CHECK_EQ(t.e[18].func->mux, 1);

    n = cfg_reply(&t, OHC_CFG_R_OK, out);
    CHECK_EQ(n, 3 + 23);
    CHECK_EQ(out[0], OHC_CFG_VERSION);
    CHECK_EQ(out[2], 23);
}

static void lm3s_hc800(void)
{
    cfg_hc800();
    CHECK_EQ(cfg_parse(&part_lm3s1162, frame, flen, &t), OHC_CFG_R_OK);
    CHECK_EQ(t.count, 2 + 14 + 6 + 1 + 4 + 4 + 1);
    for (int i = 0; i < t.count; i++)
        CHECK_EQ(t.e[i].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[0].func->unit, 1);     /* PWM1 */
    CHECK_EQ(t.e[1].func->unit, 5);     /* PWM5 */

    /* a gate to a carrier nobody configured routes nothing */
    start();
    add(OHC_CFG_IR_CARRIER, 0, D, 1, 0);
    add(OHC_CFG_IR_GATE, 0, C, 7, OHC_CFG_F_ACTIVE_LOW | OHC_CFG_F_CARRIER(0));
    add(OHC_CFG_IR_GATE, 0, C, 6, OHC_CFG_F_ACTIVE_LOW | OHC_CFG_F_CARRIER(1));
    add(OHC_CFG_IR_GATE, 0, C, 5, OHC_CFG_F_CARRIER(0));     /* same bit, same carrier */
    add(OHC_CFG_IR_CARRIER, 1, D, 4, 0);                     /* CCP3, not PWM */
    add(OHC_CFG_HOLD, 0, F, 4, OHC_CFG_F_ACTIVE_LOW);
    cfg_parse(&part_lm3s1162, frame, flen, &t);
    CHECK_EQ(t.e[1].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[2].status, OHC_CFG_S_PAIR);
    CHECK_EQ(t.e[3].status, OHC_CFG_S_INDEX);
    CHECK_EQ(t.e[4].status, OHC_CFG_S_FUNC);
    CHECK_EQ(t.e[5].status, OHC_CFG_S_FLAGS);
}

static void tm4c_naks(void)
{
    const part_desc *p = &part_tm4c1231d5;

    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, A, 2, 0), OHC_CFG_S_FUNC);     /* no timer on PA2 */
    CHECK_EQ(one(p, OHC_CFG_HEARTBEAT, 1, C, 7, 0), OHC_CFG_S_INDEX);
    CHECK_EQ(one(p, OHC_CFG_UART_MUX_A, 0, E, 2, OHC_CFG_F_HIGH), OHC_CFG_S_FLAGS);
    CHECK_EQ(one(p, 13, 0, A, 2, 0), OHC_CFG_S_KIND);
    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, A, 0, 0), OHC_CFG_S_RESERVED); /* the host link */
    CHECK_EQ(one(p, OHC_CFG_RELAY, 0, C, 1, 0), OHC_CFG_S_RESERVED);  /* JTAG */
    CHECK_EQ(one(p, OHC_CFG_RELAY, 0, E, 6, 0), OHC_CFG_S_PIN);       /* PE6 is not bonded */
    CHECK_EQ(one(p, OHC_CFG_RELAY, 0, G, 0, 0), OHC_CFG_S_PIN);
    CHECK_EQ(one(p, OHC_CFG_RELAY, 0, A, 9, 0), OHC_CFG_S_PIN);
    CHECK_EQ(one(p, OHC_CFG_IR_IN, 0, B, 4, 0), OHC_CFG_S_FUNC);      /* 16-bit timer */
    CHECK_EQ(one(p, OHC_CFG_IR_IN, 1, D, 6, 0), OHC_CFG_S_INDEX);
    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 24, D, 4, 0), OHC_CFG_S_INDEX);
    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, D, 4, OHC_CFG_F_ACTIVE_LOW), OHC_CFG_S_FLAGS);
    CHECK_EQ(one(p, OHC_CFG_CONTACT, 0, A, 2, OHC_CFG_F_PULL_UP | OHC_CFG_F_PULL_DOWN),
             OHC_CFG_S_FLAGS);
    CHECK_EQ(one(p, 0xff, 0, A, 2, 0), OHC_CFG_S_KIND);
    CHECK_EQ(one(p, 0, 0, A, 2, 0), OHC_CFG_S_KIND);
    CHECK_EQ(one(p, OHC_CFG_UART_RX, 0, E, 4, 0), OHC_CFG_S_PAIR);    /* RX without TX */
    CHECK_EQ(one(p, OHC_CFG_UART_TX, 7, E, 5, 0), OHC_CFG_S_INDEX);   /* 7 user ports: 0..6 */

    /* PB6 and PF0 are both T0CCP0: one timer half, one carrier */
    start();
    add(OHC_CFG_IR_OUT, 0, B, 6, 0);
    add(OHC_CFG_IR_OUT, 1, F, 0, 0);
    cfg_parse(p, frame, flen, &t);
    CHECK_EQ(t.e[0].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[1].status, OHC_CFG_S_BUSY);

    /* one pin, two uses: the second loses */
    start();
    add(OHC_CFG_IR_OUT, 7, C, 4, 0);
    add(OHC_CFG_UART_RX, 0, C, 4, 0);
    add(OHC_CFG_UART_TX, 0, C, 5, 0);
    cfg_parse(p, frame, flen, &t);
    CHECK_EQ(t.e[0].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[1].status, OHC_CFG_S_BUSY);
    CHECK_EQ(t.e[2].status, OHC_CFG_S_PAIR);

    /* the same index twice */
    start();
    add(OHC_CFG_RELAY, 3, F, 1, 0);
    add(OHC_CFG_RELAY, 3, F, 3, 0);
    cfg_parse(p, frame, flen, &t);
    CHECK_EQ(t.e[1].status, OHC_CFG_S_INDEX);

    /* RX on UART5, TX on UART7 */
    start();
    add(OHC_CFG_UART_RX, 0, E, 4, 0);
    add(OHC_CFG_UART_TX, 0, E, 1, 0);
    cfg_parse(p, frame, flen, &t);
    CHECK_EQ(t.e[0].status, OHC_CFG_S_PAIR);
    CHECK_EQ(t.e[1].status, OHC_CFG_S_PAIR);

    /* PB0/PB1 take UART1, so PC4/PC5 must end up on UART4 rather than share it */
    start();
    add(OHC_CFG_UART_RX, 0, B, 0, 0);
    add(OHC_CFG_UART_TX, 0, B, 1, 0);
    add(OHC_CFG_UART_RX, 1, C, 4, 0);
    add(OHC_CFG_UART_TX, 1, C, 5, 0);
    cfg_parse(p, frame, flen, &t);
    for (int i = 0; i < 4; i++)
        CHECK_EQ(t.e[i].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[0].func->unit, 1);
    CHECK_EQ(t.e[2].func->unit, 4);

    /* the frame itself */
    start();
    add(OHC_CFG_RELAY, 0, F, 1, 0);
    frame[0] = 2;
    CHECK_EQ(cfg_parse(p, frame, flen, &t), OHC_CFG_R_VERSION);
    frame[0] = OHC_CFG_VERSION;
    CHECK_EQ(cfg_parse(p, frame, flen - 1, &t), OHC_CFG_R_LENGTH);
    CHECK_EQ(cfg_parse(p, frame, 1, &t), OHC_CFG_R_LENGTH);
    frame[1] = OHC_CFG_MAX_ENTRIES + 1;
    CHECK_EQ(cfg_parse(p, frame, OHC_CFG_HDR + (OHC_CFG_MAX_ENTRIES + 1) * OHC_CFG_ENTRY, &t),
             OHC_CFG_R_LENGTH);
    start();
    CHECK_EQ(cfg_parse(p, frame, flen, &t), OHC_CFG_R_OK);  /* empty: everything off */
    CHECK_EQ(t.count, 0);
}

static void lm3s(void)
{
    const part_desc *p = &part_lm3s1162;

    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, D, 0, 0), OHC_CFG_S_OK);       /* PWM0 */
    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, H, 1, 0), OHC_CFG_S_OK);       /* PWM3 */
    CHECK_EQ(one(p, OHC_CFG_IR_OUT, 0, D, 4, 0), OHC_CFG_S_FUNC);     /* CCP3, not PWM */
    CHECK_EQ(one(p, OHC_CFG_IR_IN, 0, D, 4, 0), OHC_CFG_S_OK);        /* any pin */
    CHECK_EQ(one(p, OHC_CFG_IR_IN, 0, B, 7, 0), OHC_CFG_S_RESERVED);  /* TRST */
    CHECK_EQ(one(p, OHC_CFG_UART_TX, 2, D, 3, 0), OHC_CFG_S_INDEX);
    start();
    add(OHC_CFG_UART_RX, 0, D, 2, 0);
    add(OHC_CFG_UART_TX, 0, D, 3, 0);
    cfg_parse(p, frame, flen, &t);
    CHECK_EQ(t.e[0].status, OHC_CFG_S_OK);
    CHECK_EQ(t.e[1].func->unit, 1);
}

static void same(void)
{
    static cfg_table a, b;

    cfg_ea3();
    cfg_parse(&part_tm4c1231d5, frame, flen, &a);
    cfg_parse(&part_tm4c1231d5, frame, flen, &b);
    CHECK(cfg_same(&a, &b));
    frame[OHC_CFG_HDR + 3] = 5;     /* entry 0 on another pin */
    cfg_parse(&part_tm4c1231d5, frame, flen, &b);
    CHECK(!cfg_same(&a, &b));
}

void test_config(void)
{
    tm4c_ea3();
    lm3s_hc800();
    tm4c_naks();
    lm3s();
    same();
}
