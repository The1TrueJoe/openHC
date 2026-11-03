/* SPDX-License-Identifier: MIT */
/* IR transmit scheduling, receive demodulation and capture framing. Hardware-free. */
#ifndef OHC_IR_H
#define OHC_IR_H

#include <stdbool.h>
#include <stdint.h>

#include "ohc-iomcu-proto.h"

typedef enum {
    IR_OK = 0,
    IR_ERR_SHORT,       /* no room for the fixed header, or an odd byte */
    IR_ERR_CARRIER,     /* word 0, or out of range */
    IR_ERR_PAIRS,       /* once/repeat counts disagree with the body */
    IR_ERR_EMPTY,
} ir_status;

typedef struct {
    uint8_t mode;               /* < 2 once; else passes, 0xff until stopped */
    uint32_t mask;
    uint32_t carrier_hz;
    uint32_t carrier_ticks;     /* clock ticks per carrier period */
    const uint8_t *words;       /* BE u16, still in the payload */
    uint16_t n_words;
    uint16_t repeat_from;       /* first word of the repeat sequence */
} ir_tx_job;

typedef struct {
    uint16_t index;
    uint8_t pass;
} ir_tx_cursor;

ir_status ir_tx_parse(const uint8_t *payload, uint16_t len, uint32_t clock_hz, ir_tx_job *job);

/* Next burst: false when done. Marks are the even words of each pass. */
bool ir_tx_next(const ir_tx_job *job, ir_tx_cursor *cur, bool *mark, uint32_t *ticks);

/* ---- receive ---- */

#define IR_RX_IDLE_US 25000u    /* silence that ends a code */
#define IR_RX_NOMINAL_HZ 38000u /* carrier assumed for an envelope receiver */
#define IR_RX_MIN_WORDS 3u      /* shorter is noise: the shortest code is mark, space, mark */

typedef struct {
    uint32_t clock_hz;
    uint32_t last;          /* previous edge */
    uint32_t start;         /* current burst */
    uint32_t sum, n;        /* carrier intervals seen */
    uint32_t carrier;       /* ticks per period, 0 until measured */
    uint16_t *out;
    uint16_t cap, count;
    bool demod;
    bool started, in_mark, overflow;
} ir_rx;

void ir_rx_init(ir_rx *rx, uint16_t *out, uint16_t cap, uint32_t clock_hz, bool demod);

/* Carrier receiver: one edge per carrier cycle. */
void ir_rx_edge(ir_rx *rx, uint32_t now);
/* Envelope receiver: an edge at every burst boundary, `mark` being the new level. */
void ir_rx_level(ir_rx *rx, uint32_t now, bool mark);
/* True once a code has ended (IR_RX_IDLE_US of silence) and holds bursts. */
bool ir_rx_poll(ir_rx *rx, uint32_t now);
/* Carrier period in ticks, measured or nominal. */
uint32_t ir_rx_carrier(const ir_rx *rx);

/* ---- IRIN_CAPTURED framing ---- */

#define IR_CAP_FRAME_BYTES (2u + OHC_IRIN_WORDS * 2u)

typedef struct {
    uint16_t carrier;           /* reported period, OHC_IRIN_TICK_HZ ticks */
    const uint16_t *words;
    uint16_t count;
    uint16_t sent;
} ir_cap;

/* Next chunk into out (IR_CAP_FRAME_BYTES); 0 when all sent. */
uint16_t ir_cap_next(ir_cap *cap, uint8_t *out);
uint16_t ir_cap_word(bool mark, uint32_t periods);

#endif
