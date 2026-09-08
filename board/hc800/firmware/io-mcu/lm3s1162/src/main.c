/* openHC IO-processor firmware for the HC-800's LM3S1162 — application.
 *
 * Answers the host over UART0 at 115200 with the same wire protocol the stock
 * firmware speaks, so the host side needs no changes. That protocol is not
 * assumed: a live HC-800 was asked FIRMWARE_VERSION_GET and PRODUCT_NAME on
 * /dev/ttyS3 and replied
 *
 *   10 02 35 01 02 00 08 "03.26.15"                            33
 *   10 02 25 02 02 00 1b "c4:ir_processor:c4-ir01-i2c"         6c
 *
 * which confirms, on this board specifically: 115200 (not the EA's 460800),
 * reply opcode = request + 1, flags bit 1 = response, and the negated-8-bit-sum
 * checksum. ohc_proto.c is shared with the EA firmware and needed no change.
 *
 * WHAT IS AND IS NOT WIRED. The link, identity and request/response plumbing
 * are real. IR transmit builds a correct carrier on the PWM module but does not
 * yet reach a pin, and relays/contacts are tracked but not driven, because the
 * GPIO pin map is not decoded — see the header of ir_pins.h. Those are gated on
 * OHC_IR_PINS_KNOWN / OHC_RELAY_PINS_KNOWN so this image is safe to flash on a
 * unit wired to real equipment.
 */
#include "board_profile.h"
#include "ir_engine.h"
#include "ir_pins.h"
#include "lm3s.h"
#include "ohc_proto.h"

#include <string.h>

/* Ours names the board, so a running unit can be identified without guessing
 * which image was flashed. The host only logs this string, it does not parse
 * it — the stock value is "c4:ir_processor:c4-ir01-i2c". */
#define PRODUCT  "c4:ir_processor:ohc-" OHC_BOARD_NAME
#define FIRMWARE "0.1.0"

extern uint32_t g_dc0;         /* latched in startup.c before anything runs */
extern uint32_t g_did1;
extern uint32_t g_sysclk_hz;   /* measured in lm3s_clock_init() */

static uint8_t txbuf[OHC_MAX_PAYLOAD * 2 + 32];

static void send_frame(uint8_t opcode, uint8_t seq, uint8_t flags,
                       const uint8_t *payload, uint16_t len)
{
    size_t n = ohc_encode(opcode, seq, flags, payload, len, txbuf, sizeof txbuf);
    for (size_t i = 0; i < n; i++) {
        lm3s_uart_putc(UART_HOST_BASE, txbuf[i]);
    }
}

static void reply(const ohc_frame *req, const uint8_t *payload, uint16_t len)
{
    send_frame((uint8_t)(req->opcode + 1u), req->seq, OHC_FLAG_RESPONSE, payload, len);
}

/* ── state ────────────────────────────────────────────────────────────────── */
static uint8_t  ir_pin_mask;
static uint8_t  ir_capture_armed;
static bool     ir_stop_requested;
static uint32_t relay_state;      /* bit N = relay N energised (tracked only) */
static ohc_rx   rx;

static void ir_transmit(const ir_tx_job *job)
{
    ir_tx_cursor cur = { 0, 0 };
    bool on;
    uint32_t ticks;
    uint32_t mask = job->output_mask & OHC_IR_OUT_MASK;

    ir_stop_requested = false;

    for (uint8_t chan = 0; chan < OHC_IR_OUT_COUNT; chan++) {
        if (mask & (1u << chan)) {
            ir_carrier_configure(chan, job->carrier_ticks);
        }
    }

    /* Blocking, like the EA firmware and for the same reason: this MCU has one
     * job at a time and a main-loop burst schedule has no interrupt latency in
     * it. The UART is still drained while waiting so IROUT_STOP_RAMP can land
     * mid-transmission — without that a repeat=0xFF "ramp" code would transmit
     * forever with no way to stop it. */
    while (!ir_stop_requested && ir_tx_next(job, &cur, &on, &ticks)) {
        for (uint8_t chan = 0; chan < OHC_IR_OUT_COUNT; chan++) {
            if (mask & (1u << chan)) {
                ir_carrier_set(chan, on);
            }
        }
        ir_burst_timer_start(ticks);
        while (!ir_burst_timer_expired()) {
            int c = lm3s_uart_getc(UART_HOST_BASE);
            if (c >= 0) {
                uint8_t b = (uint8_t)c;
                ohc_rx_feed(&rx, &b, 1);
            }
        }
    }
    for (uint8_t chan = 0; chan < OHC_IR_OUT_COUNT; chan++) {
        if (mask & (1u << chan)) {
            ir_carrier_set(chan, false);
        }
    }
}

static void put_be32(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)(v >> 24); p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);  p[3] = (uint8_t)v;
}

static void on_frame(const ohc_frame *f, void *user)
{
    (void)user;
    switch (f->opcode) {
    case OHC_OP_PRODUCT_NAME: {
        /* The identity string carries what the part turned out to be. lm3s.ld
         * has to assume a conservative 8 KB SRAM because nothing in the vendor
         * images reveals the real size; DC0 does, and this is how that fact
         * gets off the board. Format:
         *   "c4:ir_processor:ohc-hc800 flash=64K sram=NNK clk=NNNNNNNN" */
        static char buf[96];
        uint32_t flash_kb = (SYSCTL_DC0_FLASHSZ(g_dc0) * 2048u) / 1024u;
        uint32_t sram_kb  = (SYSCTL_DC0_SRAMSZ(g_dc0) * 256u) / 1024u;
        size_t n = 0;
        const char *p = PRODUCT;
        while (*p && n < sizeof buf - 40) buf[n++] = *p++;
        /* Tiny decimal formatter — libc_min has no snprintf and this is the
         * only place that needs one. */
        const char *labels[3] = { " flash=", " sram=", " clk=" };
        uint32_t vals[3] = { flash_kb, sram_kb, g_sysclk_hz };
        for (int i = 0; i < 3; i++) {
            for (const char *l = labels[i]; *l && n < sizeof buf - 12; l++) buf[n++] = *l;
            uint32_t v = vals[i];
            char tmp[12]; int t = 0;
            do { tmp[t++] = (char)('0' + v % 10u); v /= 10u; } while (v && t < 11);
            while (t--) buf[n++] = tmp[t];
        }
        reply(f, (const uint8_t *)buf, (uint16_t)n);
        break;
    }

    case OHC_OP_FIRMWARE_VERSION_GET:
        reply(f, (const uint8_t *)FIRMWARE, (uint16_t)strlen(FIRMWARE));
        break;

    case OHC_OP_CONTACT_GET: {
        /* u32 bitmask, bit N = contact N, CLOSED reads 1 — established on a
         * live EA3 by shorting its input, and the wire format is shared. The
         * host POLLS this; nothing here pushes changes. Four contacts on this
         * board, per the panel. */
        uint8_t st[4] = { 0, 0, 0, 0 };
#if OHC_RELAY_PINS_KNOWN
        uint32_t bits = 0;
        for (uint8_t i = 0; i < OHC_CONTACT_COUNT; i++) {
            if (GPIO_DATA(CONTACT_PINS[i].gpio_base, CONTACT_PINS[i].pin_mask)) {
                bits |= 1u << i;
            }
        }
        put_be32(st, bits);
#else
        /* Pins unknown — report all-clear, which gives the host a definite
         * answer rather than a timeout. */
        (void)put_be32;
#endif
        reply(f, st, sizeof st);
        break;
    }

    case OHC_OP_RELAY_GET:
    case OHC_OP_RELAY_TOGGLE: {
        /* Four relays. RELAY_TOGGLE's payload selects which; we track the bits
         * so the host sees a coherent device, but DRIVE NOTHING until the pin
         * map is decoded — a relay here may be switching a real load, and
         * guessing a pin to find out is not an acceptable experiment. */
        if (f->opcode == OHC_OP_RELAY_TOGGLE && f->length >= 1) {
            relay_state ^= (uint32_t)f->payload[0] & ((1u << OHC_RELAY_COUNT) - 1u);
#if OHC_RELAY_PINS_KNOWN
            for (uint8_t i = 0; i < OHC_RELAY_COUNT; i++) {
                GPIO_DATA(RELAY_PINS[i].gpio_base, RELAY_PINS[i].pin_mask) =
                    (relay_state & (1u << i)) ? RELAY_PINS[i].pin_mask : 0u;
            }
#endif
        }
        uint8_t st[2] = { (uint8_t)(relay_state & 0xFFu), 0 };
        send_frame(OHC_OP_RELAY_STATE, f->seq, OHC_FLAG_RESPONSE, st, sizeof st);
        break;
    }

    case OHC_OP_IRIN_SET_CAPTURE:
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
        break;

    case OHC_OP_IROUT_SEND: {
        ir_tx_job job;
        ir_status st = ir_tx_parse(f->payload, f->length, g_sysclk_hz, &job);
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
        static const uint8_t r[4] = { 0x00, 0x07, 0x00, 0x00 };
        send_frame(OHC_OP_AUTO_BAUD_RESP, f->seq, OHC_FLAG_RESPONSE, r, sizeof r);
        break;
    }

    default:
        /* Unknown opcodes are dropped in silence, as the stock dispatcher does. */
        break;
    }
}

int main(void)
{
    lm3s_clock_init();

    /* No GPIO mux for UART0: the bootloader already muxed and used those pins to
     * receive this image, and we do not know which they are. Re-init the UART
     * registers only. See the note in hal_lm3s.c. */
    lm3s_clock_gate(&SYSCTL_RCGC1, RCGC1_UART(0));
    lm3s_uart_init(UART_HOST_BASE, OHC_HOST_BAUD);

    ohc_rx_init(&rx, on_frame, 0);

    /* The host syncs by sending a bare 0x55 0x55 — not a framed command — and
     * expects AUTO_BAUD_RESP back. Tooling uses that to decide whether the
     * application is running at all, so without it a healthy firmware looks
     * dead. Watch for the pair ahead of the framing decoder, which would
     * otherwise discard it as noise. */
    uint8_t sync_run = 0;

    for (;;) {
        int c = lm3s_uart_getc(UART_HOST_BASE);
        if (c >= 0) {
            uint8_t b = (uint8_t)c;
            if (b == 0x55u) {
                if (++sync_run >= 2u) {
                    sync_run = 0;
                    static const uint8_t r[4] = { 0x00, 0x07, 0x00, 0x00 };
                    send_frame(OHC_OP_AUTO_BAUD_RESP, 0, OHC_FLAG_RESPONSE, r, sizeof r);
                }
                continue;
            }
            sync_run = 0;
            ohc_rx_feed(&rx, &b, 1);
        }
        /* TODO(hw): IR capture. Needs the receiver pin, which is part of the
         * same undecoded config block as the output pins. */
        (void)ir_capture_armed;
        (void)ir_pin_mask;
        (void)g_did1;
    }
}
