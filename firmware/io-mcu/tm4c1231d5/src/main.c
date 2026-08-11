/* openHomeController IO-processor firmware — application.
 *
 * Answers the host over UART0 at 460800 with the same wire protocol the stock
 * firmware uses, so ohc-iod talks to it unchanged.
 *
 * What is implemented: the link, identity, and the request/response plumbing.
 * IR transmit and capture are stubbed at the hardware edge and marked below —
 * they need timer work that cannot be validated without the board in hand.
 */
#include "ir_engine.h"
#include "ir_pins.h"
#include "ohc_proto.h"
#include "tm4c.h"

#include <string.h>

#define PRODUCT  "c4:io_processor:ohc-ir04"
#define FIRMWARE "0.1.0"

static uint8_t txbuf[OHC_MAX_PAYLOAD * 2 + 32];

static void send_frame(uint8_t opcode, uint8_t seq, uint8_t flags,
                       const uint8_t *payload, uint16_t len)
{
    size_t n = ohc_encode(opcode, seq, flags, payload, len, txbuf, sizeof txbuf);
    for (size_t i = 0; i < n; i++) {
        tm4c_uart_putc(UART0_BASE, txbuf[i]);
    }
}

static void reply(const ohc_frame *req, const uint8_t *payload, uint16_t len)
{
    send_frame((uint8_t)(req->opcode + 1u), req->seq, OHC_FLAG_RESPONSE, payload, len);
}

/* ── IR state ─────────────────────────────────────────────────────────────── */
static uint8_t  ir_pin_mask;      /* set by IR_PIN_STATE_SET; 0 = capture off */
static uint8_t  ir_capture_armed;
static bool     ir_stop_requested;
static ohc_rx   rx;               /* fed during transmit so a stop can arrive */

/* Play a parsed job out of the IR pin.
 *
 * Blocking, deliberately: this MCU has one job at a time, and a burst schedule
 * driven from the main loop has no interrupt latency in it. The UART is still
 * drained while waiting, so IROUT_STOP_RAMP can land mid-transmission — without
 * that a repeat=0xFF ("ramp") code would transmit forever with no way to stop it.
 */
static void ir_transmit(const ir_tx_job *job)
{
    ir_tx_cursor cur = { 0, 0 };
    bool on;
    uint32_t ticks;

    ir_stop_requested = false;

    /* output_mask selects channels by bit position; configure every one that is
     * both selected and real, then drive them together. Transmitting the same
     * code on several emitters at once is the normal case — one AV rack, several
     * boxes — and the vendor protocol models it as a mask for exactly that. */
    uint32_t mask = job->output_mask;
    for (uint8_t chan = 0; chan < IR_CHANNEL_COUNT; chan++) {
        if (mask & (1u << chan)) {
            ir_carrier_configure(chan, job->carrier_ticks);
        }
    }

    while (!ir_stop_requested && ir_tx_next(job, &cur, &on, &ticks)) {
        for (uint8_t chan = 0; chan < IR_CHANNEL_COUNT; chan++) {
            if (mask & (1u << chan)) {
                ir_carrier_set(chan, on);
            }
        }
        ir_burst_timer_start(ticks);
        while (!ir_burst_timer_expired()) {
            int c = tm4c_uart_getc(UART0_BASE);
            if (c >= 0) {
                uint8_t b = (uint8_t)c;
                ohc_rx_feed(&rx, &b, 1);   /* may set ir_stop_requested */
            }
        }
    }
    for (uint8_t chan = 0; chan < IR_CHANNEL_COUNT; chan++) {
        if (mask & (1u << chan)) {
            ir_carrier_set(chan, false);
        }
    }
}

static void on_frame(const ohc_frame *f, void *user)
{
    (void)user;
    switch (f->opcode) {
    case OHC_OP_PRODUCT_NAME:
        reply(f, (const uint8_t *)PRODUCT, (uint16_t)strlen(PRODUCT));
        break;

    case OHC_OP_FIRMWARE_VERSION_GET:
        reply(f, (const uint8_t *)FIRMWARE, (uint16_t)strlen(FIRMWARE));
        break;

    case OHC_OP_CONTACT_GET: {
        /* EA1 wires no contacts; report all-clear rather than staying silent so
         * the host gets a definite answer instead of a timeout. */
        static const uint8_t none[4] = { 0, 0, 0, 0 };
        reply(f, none, sizeof none);
        break;
    }

    case OHC_OP_RELAY_GET:
    case OHC_OP_RELAY_TOGGLE: {
        static const uint8_t off[2] = { 0, 0 };
        send_frame(OHC_OP_RELAY_STATE, f->seq, OHC_FLAG_RESPONSE, off, sizeof off);
        break;
    }

    case OHC_OP_IRIN_SET_CAPTURE:
        /* Resets capture state AND clears the pin mask — same semantics as the
         * stock firmware, which is why the host must send IR_PIN_STATE_SET
         * afterwards to actually enable an input. */
        ir_pin_mask = 0;
        ir_capture_armed = 1;
        break;

    case OHC_OP_IR_PIN_STATE_SET:
        if (f->length >= 3) {
            ir_pin_mask = f->payload[2];
        }
        break;

    case OHC_OP_IR_PIN_STATE_CLEAR:
        ir_pin_mask = 0;
        break;

    case OHC_OP_IROUT_STOP_RAMP:
        ir_stop_requested = true;
        break;

    case OHC_OP_IR_MODE_SET:
        /* (mode & 0x30) selects the capture peripheral state on the stock part. */
        break;

    case OHC_OP_IROUT_SEND: {
        /* Validate and schedule here; the carrier itself still needs the timer
         * path (TODO(hal) below). Parsing is shared with the host tests, so a
         * malformed code is rejected with a distinct status rather than being
         * discovered as silence.
         */
        ir_tx_job job;
        ir_status st = ir_tx_parse(f->payload, f->length, OHC_SYSCLK_HZ, &job);
        uint8_t status;
        if (st != IR_OK) {
            status = 0x02;                     /* rejected: malformed code */
        } else {
            ir_transmit(&job);
            status = ir_stop_requested ? 0x01u : 0x00u;
        }
        send_frame(OHC_OP_IROUT_STATUS, f->seq, OHC_FLAG_RESPONSE, &status, 1);
        break;
    }

    case OHC_OP_AUTO_BAUD_GET: {
        /* The stock reply is 00 07 then a 16-bit measured carrier period. We do
         * not autobaud (the rate is fixed), so report a zero measurement. */
        static const uint8_t r[4] = { 0x00, 0x07, 0x00, 0x00 };
        send_frame(OHC_OP_AUTO_BAUD_RESP, f->seq, OHC_FLAG_RESPONSE, r, sizeof r);
        break;
    }

    default:
        /* Unknown opcodes are dropped in silence, exactly as the stock
         * dispatcher does — its chain simply falls through to a pop. */
        break;
    }
}

int main(void)
{
    tm4c_clock_init();

    /* UART0 on PA0/PA1 */
    SYSCTL_RCGCGPIO |= (1u << 0);
    SYSCTL_RCGCUART |= (1u << 0);
    while (!(SYSCTL_PRGPIO & (1u << 0))) {
    }
    while (!(SYSCTL_PRUART & (1u << 0))) {
    }
    GPIO_AFSEL(GPIOA_BASE) |= 0x3u;
    GPIO_PCTL(GPIOA_BASE) = (GPIO_PCTL(GPIOA_BASE) & 0xFFFFFF00u) | 0x11u;
    GPIO_DEN(GPIOA_BASE) |= 0x3u;

    tm4c_uart_init(UART0_BASE, OHC_HOST_BAUD);

    ohc_rx_init(&rx, on_frame, 0);

    /* The host syncs by sending a bare 0x55 0x55 — not a framed command — and
     * expects an AUTO_BAUD_RESP frame back. Tooling uses that exchange to decide
     * whether the application is running at all (ohc-io does exactly this), so
     * without it a perfectly healthy firmware looks dead. Watch for the pair
     * ahead of the framing decoder, which would discard it as noise.
     */
    uint8_t sync_run = 0;

    for (;;) {
        int c = tm4c_uart_getc(UART0_BASE);
        if (c >= 0) {
            uint8_t b = (uint8_t)c;
            if (b == 0x55u) {
                if (++sync_run >= 2u) {
                    sync_run = 0;
                    static const uint8_t r[4] = { 0x00, 0x07, 0x00, 0x00 };
                    send_frame(OHC_OP_AUTO_BAUD_RESP, 0, OHC_FLAG_RESPONSE, r, sizeof r);
                }
                continue;               /* never feed sync bytes to the decoder */
            }
            sync_run = 0;
            ohc_rx_feed(&rx, &b, 1);
        }
        /* TODO(hal): when capture is armed and the IR timer has produced a
         * complete burst list, emit it as IRIN_CAPTURED (0x97): u16 carrier
         * period in timer ticks, then u16 burst words with bit15 = carrier on. */
        (void)ir_capture_armed;
        (void)ir_pin_mask;
    }
}
