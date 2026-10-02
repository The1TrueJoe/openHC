#include "ir_engine.h"

static uint16_t be16(const uint8_t *p) { return (uint16_t)((p[0] << 8) | p[1]); }

const char *ir_status_str(ir_status s)
{
    switch (s) {
    case IR_OK:             return "ok";
    case IR_ERR_SHORT:      return "payload too short";
    case IR_ERR_TYPE:       return "only Pronto type 0000 is supported";
    case IR_ERR_CARRIER:    return "carrier out of range";
    case IR_ERR_PAIR_COUNT: return "burst pair count mismatch";
    case IR_ERR_NO_BURSTS:  return "no burst pairs";
    }
    return "unknown";
}

ir_status ir_tx_parse(const uint8_t *payload, uint16_t len, uint32_t timer_hz,
                      ir_tx_job *job)
{
    /* 6-byte header + 4 pronto words */
    if (len < 6 + 8) {
        return IR_ERR_SHORT;
    }
    job->repeat = payload[0];
    job->output_mask = ((uint32_t)payload[1] << 16) | ((uint32_t)payload[2] << 8) | payload[3];
    job->code_id = be16(&payload[4]);

    const uint8_t *pronto = &payload[6];
    if (be16(&pronto[0]) != 0) {
        return IR_ERR_TYPE;
    }
    uint16_t divisor = be16(&pronto[2]);
    if (divisor == 0) {
        return IR_ERR_CARRIER;
    }
    job->carrier_hz = IR_PRONTO_CLOCK / divisor;
    if (job->carrier_hz < IR_CARRIER_MIN_HZ || job->carrier_hz > IR_CARRIER_MAX_HZ) {
        return IR_ERR_CARRIER;
    }
    /* Round to nearest so the carrier is not systematically slow. */
    job->carrier_ticks = (timer_hz + job->carrier_hz / 2u) / job->carrier_hz;
    if (job->carrier_ticks == 0) {
        return IR_ERR_CARRIER;
    }

    job->once_pairs = be16(&pronto[4]);
    job->repeat_pairs = be16(&pronto[6]);
    uint32_t declared = (uint32_t)job->once_pairs + job->repeat_pairs;
    if (declared == 0) {
        return IR_ERR_NO_BURSTS;
    }
    if (declared > IR_MAX_PAIRS) {
        return IR_ERR_PAIR_COUNT;
    }
    /* Body must hold exactly 2 words per declared pair — the stock firmware
     * cross-checks this too, and a mismatch means a truncated or lying frame. */
    uint16_t body_words = (uint16_t)((len - 6 - 8) / 2);
    if (body_words != declared * 2u) {
        return IR_ERR_PAIR_COUNT;
    }
    job->bursts = &pronto[8];
    job->burst_words = body_words;
    return IR_OK;
}

bool ir_tx_next(const ir_tx_job *job, ir_tx_cursor *cur,
                bool *carrier_on, uint32_t *ticks)
{
    uint16_t once_words = (uint16_t)(job->once_pairs * 2u);
    uint16_t repeat_words = (uint16_t)(job->repeat_pairs * 2u);

    if (cur->index >= job->burst_words) {
        /* End of a pass. repeat==0xFF means keep going until told to stop;
         * otherwise `repeat` counts total transmissions. */
        bool forever = (job->repeat == 0xFF);
        uint8_t total = job->repeat == 0 ? 1u : job->repeat;
        if (!forever && cur->pass + 1u >= total) {
            return false;
        }
        if (cur->pass < 0xFF) {
            cur->pass++;
        }
        /* Subsequent passes replay only the repeat sequence when there is one —
         * that is what the once/repeat split is for. */
        cur->index = repeat_words ? once_words : 0u;
        if (cur->index >= job->burst_words) {
            return false;
        }
    }

    uint16_t word = be16(&job->bursts[cur->index * 2u]);
    /* Even index = mark. Position carries mark/space in Pronto; the capture
     * direction flags it with bit 15 instead, which is easy to conflate. */
    *carrier_on = ((cur->index & 1u) == 0u);
    *ticks = (uint32_t)(word & 0x3FFFu) * job->carrier_ticks;
    cur->index++;
    return true;
}

uint16_t ir_cap_word(bool carrier_on, uint32_t carrier_periods)
{
    if (carrier_periods > 0x3FFFu) {
        carrier_periods = 0x3FFFu;
    }
    return (uint16_t)((carrier_on ? 0x8000u : 0u) | carrier_periods);
}

uint16_t ir_cap_next_frame(ir_cap_source *src, uint8_t *out)
{
    if (src->emitted >= src->count) {
        return 0;
    }
    uint16_t remaining = (uint16_t)(src->count - src->emitted);
    uint16_t take = remaining > IR_CAP_WORDS_PER_FRAME ? IR_CAP_WORDS_PER_FRAME : remaining;

    /* Every chunk repeats the carrier header — the host uses it to key the
     * frames together, and a chunk without it is undecodable on its own. */
    out[0] = (uint8_t)(src->carrier_ticks >> 8);
    out[1] = (uint8_t)src->carrier_ticks;
    for (uint16_t i = 0; i < take; i++) {
        uint16_t w = src->bursts[src->emitted + i];
        out[2 + i * 2] = (uint8_t)(w >> 8);
        out[3 + i * 2] = (uint8_t)w;
    }
    src->emitted = (uint16_t)(src->emitted + take);
    return (uint16_t)(2u + take * 2u);
}

/* ── receive demodulator ──────────────────────────────────────────────── */

/* A mark is "still going" while carrier edges keep arriving. Allow a generous
 * multiple of the carrier period before declaring a space: real receivers drop
 * the odd cycle, and treating one missed edge as a gap would shred every mark
 * into fragments. */
#define IR_RX_GAP_MULT 4u
/* Until the carrier has been measured, fall back to a fixed gap. 200 us is
 * comfortably longer than any carrier period from 20 kHz (50 us) up, and
 * comfortably shorter than the ~560 us shortest burst in common protocols. */
#define IR_RX_BOOTSTRAP_GAP_US 200u

static void push_burst(ir_rx *rx, bool mark, uint32_t ticks)
{
    if (rx->count >= rx->cap) {
        rx->overflow = true;
        return;
    }
    uint32_t periods = rx->carrier_ticks ? (ticks + rx->carrier_ticks / 2u) / rx->carrier_ticks
                                         : ticks;
    rx->out[rx->count++] = ir_cap_word(mark, periods);
}

void ir_rx_init(ir_rx *rx, uint16_t *out, uint16_t cap)
{
    rx->last_edge = 0;
    rx->burst_start = 0;
    rx->carrier_sum = 0;
    rx->carrier_n = 0;
    rx->carrier_ticks = 0;
    rx->out = out;
    rx->cap = cap;
    rx->count = 0;
    rx->in_mark = false;
    rx->started = false;
    rx->overflow = false;
}

static uint32_t gap_threshold(const ir_rx *rx, uint32_t timer_hz)
{
    if (rx->carrier_ticks) {
        return (uint32_t)rx->carrier_ticks * IR_RX_GAP_MULT;
    }
    return (timer_hz / 1000000u) * IR_RX_BOOTSTRAP_GAP_US;
}

void ir_rx_edge(ir_rx *rx, uint32_t now)
{
    if (!rx->started) {
        rx->started = true;
        rx->in_mark = true;
        rx->burst_start = now;
        rx->last_edge = now;
        return;
    }

    uint32_t delta = now - rx->last_edge;     /* wraps correctly on u32 */
    uint32_t gap = gap_threshold(rx, OHC_IR_TIMER_HZ);

    if (delta > gap) {
        /* The carrier stopped at last_edge, so that is where the mark ended and
         * the space began — NOT `now`, which is already into the next mark. */
        if (rx->in_mark) {
            push_burst(rx, true, rx->last_edge - rx->burst_start);
            push_burst(rx, false, now - rx->last_edge);
            rx->burst_start = now;
        } else {
            rx->burst_start = now;
            rx->in_mark = true;
        }
    } else if (rx->in_mark) {
        /* Consecutive carrier cycles: average them into the carrier estimate.
         * Averaging rather than taking one interval keeps a single jittery edge
         * from setting the frequency for the whole code. */
        rx->carrier_sum += delta;
        rx->carrier_n++;
        if (rx->carrier_n >= 8u && rx->carrier_ticks == 0u) {
            rx->carrier_ticks = (uint16_t)(rx->carrier_sum / rx->carrier_n);
        }
    }
    rx->last_edge = now;
}

bool ir_rx_poll(ir_rx *rx, uint32_t now, uint32_t idle_ticks)
{
    if (!rx->started) {
        return false;
    }
    if ((uint32_t)(now - rx->last_edge) < idle_ticks) {
        return false;
    }
    if (rx->in_mark) {
        push_burst(rx, true, rx->last_edge - rx->burst_start);
        rx->in_mark = false;
    }
    return rx->count > 0;
}
