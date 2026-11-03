/* SPDX-License-Identifier: MIT */
/*
 * The application. Starts with nothing configured and drives no pin until the
 * host's CONFIG_SET says which pins are what; speaks Control4's opcodes so the
 * host side is the same against stock firmware. Hardware is behind hal.h.
 */
#include "app.h"
#include "config.h"
#include "hal.h"
#include "ir.h"
#include "proto.h"
#include "ring.h"

#ifndef OHC_FW_VERSION
#define OHC_FW_VERSION OHC_FW_VERSION_PREFIX "dev"
#endif

#define CAP_WORDS 512u
#define UART_TX_SIZE 256u
#define UART_RX_SIZE 128u
#define UART_RX_BATCH 48u       /* bytes that go out at once */
#define UART_RX_HOLD_MS 2u      /* or after this long */
#define HEARTBEAT_MS 100u

static const uint32_t uart_bauds[] = { 1200, 2400, 4800, 9600, 19200, 38400, 57600 };

static ohc_rx rx;
static uint8_t knocks;
static uint8_t useq;            /* sequence for unsolicited frames */

static cfg_table cfg, scratch;
static const cfg_entry *ir_out[OHC_IR_MAX_OUTPUTS];
static const cfg_entry *ir_in;
static const cfg_entry *relay[32];
static const cfg_entry *contact[32];
static uint32_t relay_on;
static const cfg_entry *carrier[OHC_CFG_CARRIERS];   /* shared carrier sources */
static const cfg_entry *heartbeat;
static uint32_t heartbeat_ms;
static bool heartbeat_level;

/* transmit: the job lives here while the burst interrupt walks it */
static uint8_t tx_buf[OHC_MAX_PAYLOAD];
static ir_tx_job job;
static ir_tx_cursor cur;
static uint32_t tx_mask;        /* outputs with their own carrier */
static uint8_t tx_carriers;     /* shared carriers, gated onto jacks */
static const cfg_entry *tx_gate[OHC_IR_MAX_OUTPUTS];
static uint8_t tx_gates;
static uint8_t tx_seq;
static volatile bool tx_active, tx_stop, tx_done;

/* receive */
static uint16_t cap_words[CAP_WORDS];
static ir_rx irrx;
static volatile bool cap_run;   /* configured, asked for, and not transmitting */
static bool cap_on;             /* asked for by the host */

struct uart {
    const cfg_entry *rx, *tx;
    const cfg_entry *mux_a, *mux_b;     /* the combo jack's IR/serial lines */
    bool on;
    ring txr, rxr;
    uint8_t txb[UART_TX_SIZE], rxb[UART_RX_SIZE];
    uint32_t rx_ms;
};
static struct uart uarts[HAL_USER_UARTS];

static void put(uint8_t b, void *user)
{
    (void)user;
    hal_host_putc(b);
}

static void send(uint8_t op, uint8_t seq, uint8_t flags, const uint8_t *p, uint16_t n)
{
    ohc_emit(op, seq, flags, p, n, put, 0);
}

static void reply(const ohc_frame *f, uint8_t op, const uint8_t *p, uint16_t n)
{
    send(op, f->seq, OHC_FLAG_RESPONSE, p, n);
}

static uint16_t str(uint8_t *out, uint16_t at, const char *s)
{
    while (*s)
        out[at++] = (uint8_t)*s++;
    return at;
}

static uint16_t dec(uint8_t *out, uint16_t at, uint32_t v)
{
    char tmp[10];
    int n = 0;

    do {
        tmp[n++] = (char)('0' + v % 10u);
        v /= 10u;
    } while (v);
    while (n)
        out[at++] = (uint8_t)tmp[--n];
    return at;
}

static void be32(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

/* ---- capture ---- */

static void capture_update(void)
{
    bool run = ir_in && cap_on && !tx_active;

    if (run && !cap_run)
        ir_rx_init(&irrx, cap_words, CAP_WORDS, hal_clock_hz(),
                   ir_in->flags & OHC_CFG_F_DEMODULATED);
    cap_run = run;
    if (ir_in)
        hal_capture_enable(run);
}

void app_capture_edge(uint32_t t, bool level)
{
    if (!cap_run)
        return;
    if (irrx.demod)
        ir_rx_level(&irrx, t, level != !!(ir_in->flags & OHC_CFG_F_ACTIVE_LOW));
    else
        ir_rx_edge(&irrx, t);
}

static void capture_poll(void)
{
    uint8_t frame[IR_CAP_FRAME_BYTES];
    uint32_t s, c, khz;
    ir_cap cap;
    uint16_t n;
    bool done;

    if (!cap_run)
        return;
    s = hal_irq_off();
    done = ir_rx_poll(&irrx, hal_capture_now());
    if (done)
        cap_run = false;    /* hold the words while they go out */
    hal_irq_restore(s);
    if (!done)
        return;

    /* the period goes out in OHC_IRIN_TICK_HZ ticks, whatever this part's clock */
    c = ir_rx_carrier(&irrx);
    khz = hal_clock_hz() / 1000u;
    cap.carrier = (uint16_t)((c * (OHC_IRIN_TICK_HZ / 1000u) + khz / 2u) / khz);
    cap.words = cap_words;
    cap.count = irrx.count;
    cap.sent = 0;
    while ((n = ir_cap_next(&cap, frame)) != 0)
        send(OHC_OP_IRIN_CAPTURED, useq++, 0, frame, n);
    capture_update();
}

/* ---- transmit ---- */

static void gate(bool on)
{
    for (uint8_t i = 0; i < OHC_IR_MAX_OUTPUTS; i++)
        if (tx_mask & (1u << i))
            hal_carrier_gate(ir_out[i], on);
    for (uint8_t c = 0; c < OHC_CFG_CARRIERS; c++)
        if (tx_carriers & (1u << c))
            hal_carrier_gate(carrier[c], on);
}

static void route(bool on)
{
    for (uint8_t i = 0; i < tx_gates; i++) {
        const cfg_entry *g = tx_gate[i];

        hal_gpio_set(g->port, g->pin, on != !!(g->flags & OHC_CFG_F_ACTIVE_LOW));
    }
}

static void tx_end(void)
{
    hal_burst_stop();
    gate(false);
    for (uint8_t i = 0; i < OHC_IR_MAX_OUTPUTS; i++)
        if (tx_mask & (1u << i))
            hal_carrier_stop(ir_out[i]);
    for (uint8_t c = 0; c < OHC_CFG_CARRIERS; c++)
        if (tx_carriers & (1u << c))
            hal_carrier_stop(carrier[c]);
    route(false);
    tx_active = false;
}

/* The gate that routes a configured carrier to output `bit`, lowest carrier first. */
static const cfg_entry *gate_for(uint8_t bit)
{
    const cfg_entry *best = 0;

    for (uint8_t i = 0; i < cfg.count; i++) {
        const cfg_entry *g = &cfg.e[i];
        uint8_t c = OHC_CFG_F_CARRIER_OF(g->flags);

        if (g->kind == OHC_CFG_IR_GATE && g->status == OHC_CFG_S_OK && g->index == bit &&
            carrier[c] && (!best || c < OHC_CFG_F_CARRIER_OF(best->flags)))
            best = g;
    }
    return best;
}

void app_burst_isr(void)
{
    bool mark;
    uint32_t ticks;

    if (!tx_active)
        return;
    if (tx_stop || !ir_tx_next(&job, &cur, &mark, &ticks)) {
        tx_end();
        tx_done = true;
        return;
    }
    gate(mark);
    hal_burst_start(ticks);
}

static void ir_send(const ohc_frame *f)
{
    uint8_t st = OHC_IR_ST_OK;
    uint32_t mask = 0;
    uint8_t carriers = 0, gates = 0;

    if (tx_active) {
        st = OHC_IR_ST_BUSY;
    } else {
        for (uint16_t i = 0; i < f->length; i++)
            tx_buf[i] = f->payload[i];
        if (ir_tx_parse(tx_buf, f->length, hal_clock_hz(), &job) != IR_OK)
            st = OHC_IR_ST_REJECTED;
        for (uint8_t i = 0; st == OHC_IR_ST_OK && i < OHC_IR_MAX_OUTPUTS; i++) {
            const cfg_entry *g;

            if (!(job.mask & (1u << i)))
                continue;
            if (ir_out[i]) {
                mask |= 1u << i;
            } else if ((g = gate_for(i)) != 0) {
                tx_gate[gates++] = g;
                carriers |= (uint8_t)(1u << OHC_CFG_F_CARRIER_OF(g->flags));
            }
        }
        if (st == OHC_IR_ST_OK && !mask && !gates)
            st = OHC_IR_ST_REJECTED;
    }
    reply(f, OHC_OP_IROUT_STATUS, &st, 1);
    if (st != OHC_IR_ST_OK)
        return;

    tx_mask = mask;
    tx_carriers = carriers;
    tx_gates = gates;
    tx_seq = f->seq;
    cur.index = 0;
    cur.pass = 0;
    tx_stop = false;
    tx_active = true;
    capture_update();
    for (uint8_t i = 0; i < OHC_IR_MAX_OUTPUTS; i++)
        if (mask & (1u << i))
            hal_carrier_start(ir_out[i], job.carrier_ticks);
    for (uint8_t c = 0; c < OHC_CFG_CARRIERS; c++)
        if (carriers & (1u << c))
            hal_carrier_start(carrier[c], job.carrier_ticks);
    route(true);
    app_burst_isr();
}

static void tx_poll(void)
{
    uint8_t st;

    if (!tx_done)
        return;
    tx_done = false;
    st = tx_stop ? OHC_IR_ST_STOPPED : OHC_IR_ST_DONE;
    send(OHC_OP_IROUT_STATUS, tx_seq, 0, &st, 1);
    capture_update();
}

/* ---- user serial ---- */

void app_uart_rx(uint8_t port, uint8_t c)
{
    if (port < HAL_USER_UARTS && uarts[port].on) {
        ring_put(&uarts[port].rxr, c);
        uarts[port].rx_ms = hal_millis();
    }
}

/* Combo jack: 0 IR, 1 serial, 2 serial with RX/TX swapped. */
static void uart_mode(struct uart *u, uint8_t mode)
{
    if (u->mux_a)
        hal_gpio_set(u->mux_a->port, u->mux_a->pin, mode == 1);
    if (u->mux_b)
        hal_gpio_set(u->mux_b->port, u->mux_b->pin, mode == 2);
}

static void uart_swap(const ohc_frame *f)
{
    if (f->length >= 2 && f->payload[0] < HAL_USER_UARTS && uarts[f->payload[0]].on)
        uart_mode(&uarts[f->payload[0]], f->payload[1] ? 2 : 1);
}

static void uart_control(const ohc_frame *f)
{
    const uint8_t *p = f->payload;
    struct uart *u;
    uint32_t baud;

    if (f->length < 6 || p[0] >= HAL_USER_UARTS || !uarts[p[0]].rx)
        return;
    u = &uarts[p[0]];
    baud = p[2] < sizeof(uart_bauds) / sizeof(uart_bauds[0]) ? uart_bauds[p[2]] : 115200u;
    u->on = p[1] != 0;
    ring_init(&u->txr, u->txb, UART_TX_SIZE);
    ring_init(&u->rxr, u->rxb, UART_RX_SIZE);
    hal_uart_enable(p[0], u->on, baud, p[3] >= 5 && p[3] <= 8 ? p[3] : 8,
                    p[4] <= 2 ? p[4] : 0, p[5] == 2 ? 2 : 1);
    uart_mode(u, u->on ? 1 : 0);
}

static void uart_send(const ohc_frame *f)
{
    struct uart *u;

    if (f->length < 1 || f->payload[0] >= HAL_USER_UARTS)
        return;
    u = &uarts[f->payload[0]];
    if (!u->on)
        return;
    for (uint16_t i = 1; i < f->length; i++)
        if (!ring_put(&u->txr, f->payload[i]))
            break;  /* no flow control on this protocol: the rest is lost */
}

static void uart_poll(void)
{
    uint8_t buf[1 + UART_RX_BATCH];

    for (uint8_t i = 0; i < HAL_USER_UARTS; i++) {
        struct uart *u = &uarts[i];
        uint16_t n;
        int c;

        if (!u->on)
            continue;
        while ((c = ring_peek(&u->txr)) >= 0 && hal_uart_putc(i, (uint8_t)c))
            ring_get(&u->txr);
        for (;;) {
            n = ring_count(&u->rxr);
            if (!n || (n < UART_RX_BATCH && hal_millis() - u->rx_ms < UART_RX_HOLD_MS))
                break;
            buf[0] = i;
            for (n = 1; n <= UART_RX_BATCH && (c = ring_get(&u->rxr)) >= 0; n++)
                buf[n] = (uint8_t)c;
            send(OHC_OP_UART_RECEIVE, useq++, 0, buf, n);
        }
    }
}

/* ---- config ---- */

static void teardown(void)
{
    uint32_t s = hal_irq_off();

    if (tx_active)
        tx_end();
    hal_irq_restore(s);
    tx_done = false;
    cap_on = false;
    cap_run = false;
    if (ir_in)
        hal_capture_enable(false);
    for (uint8_t i = 0; i < HAL_USER_UARTS; i++)
        if (uarts[i].on)
            hal_uart_enable(i, false, 0, 8, 0, 1);
    hal_release();
    for (uint8_t i = 0; i < OHC_IR_MAX_OUTPUTS; i++)
        ir_out[i] = 0;
    for (uint8_t i = 0; i < 32; i++)
        relay[i] = contact[i] = 0;
    for (uint8_t i = 0; i < HAL_USER_UARTS; i++) {
        uarts[i].rx = uarts[i].tx = uarts[i].mux_a = uarts[i].mux_b = 0;
        uarts[i].on = false;
    }
    for (uint8_t c = 0; c < OHC_CFG_CARRIERS; c++)
        carrier[c] = 0;
    ir_in = 0;
    heartbeat = 0;
    relay_on = 0;
    tx_mask = 0;
    tx_carriers = 0;
    tx_gates = 0;
}

static void apply(void)
{
    for (uint8_t i = 0; i < cfg.count; i++) {
        const cfg_entry *e = &cfg.e[i];

        if (e->status != OHC_CFG_S_OK)
            continue;
        switch (e->kind) {
        case OHC_CFG_IR_OUT:
            ir_out[e->index] = e;
            hal_carrier_setup(e);
            break;
        case OHC_CFG_IR_IN:
            ir_in = e;
            hal_capture_setup(e);
            break;
        case OHC_CFG_RELAY:
            relay[e->index] = e;
            hal_gpio_out(e->port, e->pin, e->flags & OHC_CFG_F_ACTIVE_LOW);
            break;
        case OHC_CFG_CONTACT:
            contact[e->index] = e;
            hal_gpio_in(e->port, e->pin, e->flags);
            break;
        case OHC_CFG_UART_RX:
            if (e->index < HAL_USER_UARTS)
                uarts[e->index].rx = e;
            break;
        case OHC_CFG_UART_TX:
            if (e->index < HAL_USER_UARTS)
                uarts[e->index].tx = e;
            break;
        case OHC_CFG_IR_CARRIER:
            carrier[e->index] = e;
            hal_carrier_setup(e);
            break;
        case OHC_CFG_IR_GATE:
            hal_gpio_out(e->port, e->pin, e->flags & OHC_CFG_F_ACTIVE_LOW);   /* closed */
            break;
        case OHC_CFG_HOLD:
            hal_gpio_out(e->port, e->pin, e->flags & OHC_CFG_F_HIGH);
            break;
        case OHC_CFG_HEARTBEAT:
            heartbeat = e;
            heartbeat_level = false;
            hal_gpio_out(e->port, e->pin, false);
            break;
        case OHC_CFG_UART_MUX_A:
        case OHC_CFG_UART_MUX_B:
            if (e->index < HAL_USER_UARTS) {
                if (e->kind == OHC_CFG_UART_MUX_A)
                    uarts[e->index].mux_a = e;
                else
                    uarts[e->index].mux_b = e;
            }
            hal_gpio_out(e->port, e->pin, false);   /* the jack starts as IR */
            break;
        }
    }
    for (uint8_t i = 0; i < HAL_USER_UARTS; i++)
        if (uarts[i].rx && uarts[i].tx)
            hal_uart_setup(i, uarts[i].rx, uarts[i].tx);
        else
            uarts[i].rx = uarts[i].tx = 0;
}

static void config_set(const ohc_frame *f)
{
    uint8_t out[3 + OHC_CFG_MAX_ENTRIES];
    uint8_t r = cfg_parse(hal_part, f->payload, f->length, &scratch);

    /* the same table again (a driver rebind) leaves relays where they are */
    if (r == OHC_CFG_R_OK && !cfg_same(&scratch, &cfg)) {
        teardown();
        cfg = scratch;
        apply();
    }
    reply(f, OHC_OP_CONFIG_RESP, out, cfg_reply(&scratch, r, out));
}

/* ---- dispatch ---- */

static void relay_reply(const ohc_frame *f, bool toggle)
{
    uint8_t p[2] = { 0xff, 0 };

    if (f->length >= 1) {
        uint8_t i = f->payload[0];

        p[0] = i;
        if (i < 32 && relay[i]) {
            if (toggle) {
                relay_on ^= 1u << i;
                hal_gpio_set(relay[i]->port, relay[i]->pin,
                             !!(relay_on & (1u << i)) != !!(relay[i]->flags & OHC_CFG_F_ACTIVE_LOW));
            }
            p[1] = !!(relay_on & (1u << i));
        }
    }
    reply(f, OHC_OP_RELAY_STATE, p, 2);
}

static void contacts_reply(const ohc_frame *f)
{
    uint32_t m = 0;
    uint8_t p[4];

    for (uint8_t i = 0; i < 32; i++)
        if (contact[i] && hal_gpio_get(contact[i]->port, contact[i]->pin) !=
                          !!(contact[i]->flags & OHC_CFG_F_ACTIVE_LOW))
            m |= 1u << i;
    p[0] = (uint8_t)m;          /* little-endian: byte 0 = contacts 1..8 */
    p[1] = (uint8_t)(m >> 8);
    p[2] = (uint8_t)(m >> 16);
    p[3] = (uint8_t)(m >> 24);
    reply(f, OHC_OP_CONTACT_STATE, p, 4);
}

static void auto_baud(uint8_t seq)
{
    uint8_t p[4];

    be32(p, hal_part->host_baud);
    send(OHC_OP_AUTO_BAUD_RESP, seq, OHC_FLAG_RESPONSE, p, 4);
}

static void on_frame(const ohc_frame *f, void *user)
{
    uint8_t buf[64];
    uint16_t n;

    (void)user;
    switch (f->opcode) {
    case OHC_OP_PRODUCT_NAME:
        n = str(buf, 0, "openhc:io-mcu:");
        n = str(buf, n, hal_part->name);
        n = str(buf, n, " clk=");
        n = dec(buf, n, hal_clock_hz());
        reply(f, f->opcode + 1, buf, n);
        break;
    case OHC_OP_FIRMWARE_VERSION:
        n = str(buf, 0, OHC_FW_VERSION);
        reply(f, f->opcode + 1, buf, n);
        break;
    case OHC_OP_RELAY_GET:
        relay_reply(f, false);
        break;
    case OHC_OP_RELAY_TOGGLE:
        relay_reply(f, true);
        break;
    case OHC_OP_CONTACT_GET:
        contacts_reply(f);
        break;
    case OHC_OP_IROUT_SEND:
        ir_send(f);
        break;
    case OHC_OP_IROUT_STOP:
        tx_stop = true;
        break;
    case OHC_OP_IRIN_SET_CAPTURE:
        cap_on = true;
        cap_run = false;    /* restart from a clean state */
        capture_update();
        break;
    case OHC_OP_IR_PIN_STATE_SET:
        cap_on = f->length >= 3 && f->payload[2] != 0;
        capture_update();
        break;
    case OHC_OP_IR_PIN_STATE_CLEAR:
        cap_on = false;
        capture_update();
        break;
    case OHC_OP_UART_SET_CONTROL:
        uart_control(f);
        break;
    case OHC_OP_UART_SEND:
        uart_send(f);
        break;
    case OHC_OP_UART_MODE:
        uart_swap(f);
        break;
    case OHC_OP_AUTO_BAUD_GET:
        auto_baud(f->seq);
        break;
    case OHC_OP_CONFIG_SET:
        config_set(f);
        break;
    case OHC_OP_RESET:
        reply(f, OHC_OP_RESET_ACK, 0, 0);
        hal_host_flush();
        teardown();
        hal_reset();
        break;
    default:
        break;  /* unknown opcodes are dropped, as stock does */
    }
}

void app_init(void)
{
    ohc_rx_init(&rx, on_frame, 0);
    knocks = 0;
    cfg.count = 0;
    teardown();
}

void app_poll(void)
{
    int c;

    while ((c = hal_host_getc()) >= 0) {
        uint8_t b = (uint8_t)c;

        /* a bare 55 55 between frames is the host asking if the app is up */
        if (b == OHC_KNOCK && ohc_rx_idle(&rx)) {
            if (++knocks == 2) {
                knocks = 0;
                auto_baud(0);
            }
            continue;
        }
        knocks = 0;
        ohc_rx_feed(&rx, &b, 1);
    }
    tx_poll();
    capture_poll();
    uart_poll();
    if (heartbeat && hal_millis() - heartbeat_ms >= HEARTBEAT_MS) {
        heartbeat_ms = hal_millis();
        heartbeat_level = !heartbeat_level;
        hal_gpio_set(heartbeat->port, heartbeat->pin, heartbeat_level);
    }
}
