/* SPDX-License-Identifier: MIT */
#include "ir.h"

static uint16_t be16(const uint8_t *p) { return (uint16_t)((p[0] << 8) | p[1]); }

ir_status ir_tx_parse(const uint8_t *p, uint16_t len, uint32_t clock_hz, ir_tx_job *job)
{
    uint16_t word;

    if (len < OHC_IR_FIXED || ((len - OHC_IR_FIXED) & 1u))
        return IR_ERR_SHORT;
    job->mode = p[OHC_IR_OFF_MODE];
    job->mask = ((uint32_t)p[OHC_IR_OFF_MASK] << 16) |
                ((uint32_t)p[OHC_IR_OFF_MASK + 1] << 8) | p[OHC_IR_OFF_MASK + 2];

    /* Stock divides by this word; zero hard-faults it. Never accept it. */
    word = be16(&p[OHC_IR_OFF_CARRIER]);
    if (!word)
        return IR_ERR_CARRIER;
    job->carrier_hz = OHC_IR_PRONTO_HZ / word;
    if (job->carrier_hz < OHC_IR_CARRIER_MIN || job->carrier_hz > OHC_IR_CARRIER_MAX)
        return IR_ERR_CARRIER;
    job->carrier_ticks = (clock_hz + job->carrier_hz / 2u) / job->carrier_hz;
    if (job->carrier_ticks < 2u)
        return IR_ERR_CARRIER;

    job->words = &p[OHC_IR_FIXED];
    job->n_words = (uint16_t)((len - OHC_IR_FIXED) / 2u);
    if (!job->n_words)
        return IR_ERR_EMPTY;

    job->repeat_from = 0;
    if (job->mode >= 2) {
        uint32_t once = 2u * be16(&p[OHC_IR_OFF_ONCE]);
        uint32_t rep = 2u * be16(&p[OHC_IR_OFF_REPEAT]);

        if (once + rep != job->n_words)
            return IR_ERR_PAIRS;
        job->repeat_from = rep ? (uint16_t)once : 0u;
    }
    return IR_OK;
}

bool ir_tx_next(const ir_tx_job *job, ir_tx_cursor *cur, bool *mark, uint32_t *ticks)
{
    uint16_t first;

    if (cur->index >= job->n_words) {
        bool forever = job->mode == 0xff;
        uint8_t passes = job->mode < 2 ? 1u : job->mode;

        if (!forever && cur->pass + 1u >= passes)
            return false;
        if (cur->pass < 0xfe)
            cur->pass++;
        cur->index = job->repeat_from;
    }
    first = cur->pass ? job->repeat_from : 0u;
    *mark = ((cur->index - first) & 1u) == 0u;
    *ticks = (uint32_t)(be16(&job->words[cur->index * 2u]) & OHC_IR_DURATION_MAX) *
             job->carrier_ticks;
    if (!*ticks)
        *ticks = 1;
    cur->index++;
    return true;
}

/* ---- receive ---- */

#define IR_RX_GAP_PERIODS 4u    /* missed carrier cycles before a mark is over */
#define IR_RX_BOOT_GAP_US 200u  /* gap threshold before the carrier is measured */

void ir_rx_init(ir_rx *rx, uint16_t *out, uint16_t cap, uint32_t clock_hz, bool demod)
{
    rx->clock_hz = clock_hz;
    rx->last = rx->start = 0;
    rx->sum = rx->n = 0;
    rx->carrier = 0;
    rx->out = out;
    rx->cap = cap;
    rx->count = 0;
    rx->demod = demod;
    rx->started = rx->in_mark = rx->overflow = false;
}

uint32_t ir_rx_carrier(const ir_rx *rx)
{
    if (rx->carrier)
        return rx->carrier;
    if (!rx->demod && rx->n)
        return rx->sum / rx->n;
    return rx->clock_hz / IR_RX_NOMINAL_HZ;
}

uint16_t ir_cap_word(bool mark, uint32_t periods)
{
    if (periods > OHC_IRIN_DURATION)
        periods = OHC_IRIN_DURATION;
    return (uint16_t)((mark ? OHC_IRIN_MARK : 0u) | periods);
}

static void push(ir_rx *rx, bool mark, uint32_t ticks)
{
    uint32_t c = ir_rx_carrier(rx);

    if (rx->count >= rx->cap) {
        rx->overflow = true;
        return;
    }
    rx->out[rx->count++] = ir_cap_word(mark, (ticks + c / 2u) / c);
}

void ir_rx_edge(ir_rx *rx, uint32_t now)
{
    uint32_t gap, d;

    if (!rx->started) {
        rx->started = rx->in_mark = true;
        rx->start = rx->last = now;
        return;
    }
    d = now - rx->last;
    gap = rx->carrier ? rx->carrier * IR_RX_GAP_PERIODS
                      : rx->clock_hz / 1000000u * IR_RX_BOOT_GAP_US;
    if (d > gap) {
        if (rx->in_mark) {
            /* The last cycle ran one period past its edge; count it in the mark. */
            uint32_t c = ir_rx_carrier(rx);
            uint32_t mark = rx->last - rx->start + c;

            push(rx, true, mark);
            push(rx, false, d > c ? d - c : d);
        } else {
            push(rx, false, now - rx->start);
        }
        rx->in_mark = true;
        rx->start = now;
    } else {
        rx->sum += d;
        rx->n++;
        if (!rx->carrier && rx->n >= 8u)
            rx->carrier = rx->sum / rx->n;
    }
    rx->last = now;
}

void ir_rx_level(ir_rx *rx, uint32_t now, bool mark)
{
    if (!rx->started) {
        if (!mark)
            return;     /* a code starts with a mark */
        rx->started = rx->in_mark = true;
        rx->start = rx->last = now;
        return;
    }
    if (mark == rx->in_mark)
        return;         /* a repeated level is noise */
    push(rx, rx->in_mark, now - rx->start);
    rx->in_mark = mark;
    rx->start = rx->last = now;
}

bool ir_rx_poll(ir_rx *rx, uint32_t now)
{
    uint32_t idle = rx->clock_hz / 1000000u * IR_RX_IDLE_US;

    if (!rx->started || now - rx->last < idle)
        return false;
    if (rx->demod) {
        if (rx->in_mark)
            return false;   /* stuck low is not an end */
    } else if (rx->in_mark) {
        push(rx, true, rx->last - rx->start + ir_rx_carrier(rx));
        rx->in_mark = false;
    }
    rx->started = false;
    if (rx->count < IR_RX_MIN_WORDS) {
        rx->count = 0;      /* a lone blip, not a code */
        rx->overflow = false;
        return false;
    }
    return true;
}

uint16_t ir_cap_next(ir_cap *cap, uint8_t *out)
{
    uint16_t take, i;

    if (cap->sent >= cap->count)
        return 0;
    take = (uint16_t)(cap->count - cap->sent);
    if (take > OHC_IRIN_WORDS)
        take = OHC_IRIN_WORDS;
    out[0] = (uint8_t)(cap->carrier >> 8);
    out[1] = (uint8_t)cap->carrier;
    for (i = 0; i < take; i++) {
        uint16_t w = cap->words[cap->sent + i];

        out[2 + i * 2] = (uint8_t)(w >> 8);
        out[3 + i * 2] = (uint8_t)w;
    }
    cap->sent = (uint16_t)(cap->sent + take);
    return (uint16_t)(2u + take * 2u);
}
