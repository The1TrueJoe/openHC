/* IR transmit scheduling — hardware-free, so it is testable on the host.
 *
 * Splitting this out matters: whichever SDK the firmware ends up on, the maths
 * of turning an IROUT_SEND payload into a burst schedule is identical. The
 * hardware layer only has to supply three primitives — set the carrier
 * frequency, gate it on/off, and wait N timer ticks — and everything decided
 * here can be verified without a board.
 *
 * Payload layout (confirmed on real hardware, docs/io-mcu-firmware.md):
 *
 *   u8      repeat_count    0xFF = repeat until IROUT_STOP_RAMP
 *   u24 BE  output_mask     bit N selects IR output N
 *   u16 BE  code_id         caller handle, echoed back
 *   u16 BE  pronto[0]       type; must be 0x0000 (raw/learned)
 *   u16 BE  pronto[1]       carrier divisor: Hz = 4145146 / value
 *   u16 BE  pronto[2]       once-sequence burst PAIR count
 *   u16 BE  pronto[3]       repeat-sequence burst PAIR count
 *   u16 BE  pronto[4..]     burst durations in CARRIER PERIODS, mark first
 */
#ifndef OHC_IR_ENGINE_H
#define OHC_IR_ENGINE_H

#include <stdbool.h>
#include <stdint.h>

#define IR_PRONTO_CLOCK   4145146u
#define IR_MAX_PAIRS      256u
#define IR_CARRIER_MIN_HZ 20000u
#define IR_CARRIER_MAX_HZ 460000u

/* Timer clock the demodulator scales against. Defined here rather than pulled
 * from tm4c.h so this translation unit stays hardware-free and compiles in the
 * host tests; the firmware build can override it. */
#ifndef OHC_IR_TIMER_HZ
#define OHC_IR_TIMER_HZ 50000000u
#endif

typedef enum {
    IR_OK = 0,
    IR_ERR_SHORT,         /* payload too small to hold a header */
    IR_ERR_TYPE,          /* pronto[0] != 0 */
    IR_ERR_CARRIER,       /* divisor 0, or resulting Hz out of range */
    IR_ERR_PAIR_COUNT,    /* header count disagrees with the body, or > 256 */
    IR_ERR_NO_BURSTS,
} ir_status;

typedef struct {
    uint8_t  repeat;
    uint32_t output_mask;
    uint16_t code_id;
    uint32_t carrier_hz;
    uint32_t carrier_ticks;   /* timer ticks in one carrier period */
    uint16_t once_pairs;
    uint16_t repeat_pairs;
    const uint8_t *bursts;    /* big-endian u16 words, still in the payload */
    uint16_t burst_words;
} ir_tx_job;

/* Walks a job, emitting one burst at a time. */
typedef struct {
    uint16_t index;      /* next burst word */
    uint8_t  pass;       /* 0 = once sequence, then repeats */
} ir_tx_cursor;

/* Parse and validate. `timer_hz` is the clock the durations get scaled to. */
ir_status ir_tx_parse(const uint8_t *payload, uint16_t len, uint32_t timer_hz,
                      ir_tx_job *job);

/* Next burst. Returns false when the transmission is complete.
 * `carrier_on` alternates mark/space; `ticks` is the duration in timer ticks. */
bool ir_tx_next(const ir_tx_job *job, ir_tx_cursor *cur,
                bool *carrier_on, uint32_t *ticks);

const char *ir_status_str(ir_status s);

/* ── capture (receive) ────────────────────────────────────────────────────
 *
 * The MCU reports what it heard as IRIN_CAPTURED (0x97) payloads:
 *
 *   u16 BE  carrier period, in MCU timer ticks
 *   u16 BE  burst words: bit15 = carrier ON (mark), bits 0..13 = duration in
 *           CARRIER PERIODS
 *
 * A long code does not fit one frame, so it is CHUNKED, and every chunk repeats
 * the 2-byte carrier header. The stock firmware emits at most 100 burst words
 * per frame (a 202-byte payload); matching that exactly keeps us
 * indistinguishable from it on the wire.
 */
#define IR_CAP_WORDS_PER_FRAME 100u
#define IR_CAP_FRAME_BYTES     (2u + IR_CAP_WORDS_PER_FRAME * 2u)

typedef struct {
    uint16_t carrier_ticks;
    const uint16_t *bursts;   /* bit15 = mark, bits 0..13 = carrier periods */
    uint16_t count;
    uint16_t emitted;         /* words already packed into frames */
} ir_cap_source;

/* Pack the next chunk into `out` (needs IR_CAP_FRAME_BYTES). Returns the byte
 * count, or 0 when the whole capture has been emitted. */
uint16_t ir_cap_next_frame(ir_cap_source *src, uint8_t *out);

/* Build a burst word from a measured duration. Durations longer than 14 bits
 * cannot be represented and are clamped — better a slightly short mark than a
 * wrapped one that decodes as noise. */
uint16_t ir_cap_word(bool carrier_on, uint32_t carrier_periods);

/* ── receive demodulator ──────────────────────────────────────────────────
 *
 * The front receiver is NON-demodulating: the protocol reports a measured
 * carrier period, which is only possible if the MCU sees the carrier itself.
 * So the capture ISR gets an edge every carrier cycle — ~26 us at 38 kHz — and
 * a 100 ms code is thousands of edges. Storing them raw is not an option in
 * 24 KB of SRAM, so bursts must be reduced ONLINE: count carrier cycles while
 * they keep arriving, and close the mark when they stop.
 *
 * Feed it rising-edge timestamps from a timer's edge-time capture. It emits
 * burst words in the same encoding the host expects (bit15 = mark).
 */
typedef struct {
    uint32_t last_edge;        /* timestamp of the previous edge */
    uint32_t burst_start;      /* when the current burst began */
    uint32_t carrier_sum;      /* accumulated intervals, for averaging */
    uint32_t carrier_n;
    uint16_t carrier_ticks;    /* measured period, 0 until known */
    uint16_t *out;
    uint16_t cap;
    uint16_t count;
    bool     in_mark;
    bool     started;
    bool     overflow;         /* ran out of room; the code is incomplete */
} ir_rx;

void ir_rx_init(ir_rx *rx, uint16_t *out, uint16_t cap);

/* One captured rising edge, at `now` in timer ticks. Safe to call from an ISR. */
void ir_rx_edge(ir_rx *rx, uint32_t now);

/* Call periodically with the current time. Closes the final burst once the line
 * has been idle for `idle_ticks`, which is what ends a code — there is no
 * end marker, only silence. Returns true when a complete code is ready. */
bool ir_rx_poll(ir_rx *rx, uint32_t now, uint32_t idle_ticks);

#endif /* OHC_IR_ENGINE_H */
