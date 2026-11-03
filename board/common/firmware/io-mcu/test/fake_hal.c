/* SPDX-License-Identifier: MIT */
/* A HAL that records what the application asks of the hardware. */
#include "hal.h"
#include "test.h"

struct fake fk;
const part_desc *hal_part = &part_tm4c1231d5;

void fake_reset(const part_desc *part)
{
    memset(&fk, 0, sizeof(fk));
    fk.clock = 50000000u;
    fk.uart_fifo_room = -1;
    hal_part = part;
}

void fake_send(const uint8_t *b, size_t n)
{
    memcpy(fk.in + fk.in_len, b, n);
    fk.in_len += n;
}

struct collect {
    ohc_frame *out;
    uint8_t (*store)[OHC_MAX_PAYLOAD];
    int n, max;
};

static void grab(const ohc_frame *f, void *user)
{
    struct collect *c = user;

    if (c->n >= c->max)
        return;
    memcpy(c->store[c->n], f->payload, f->length);
    c->out[c->n] = *f;
    c->out[c->n].payload = c->store[c->n];
    c->n++;
}

int fake_frames(ohc_frame *out, uint8_t store[][OHC_MAX_PAYLOAD], int max)
{
    static ohc_rx rx;
    struct collect c = { out, store, 0, max };

    ohc_rx_init(&rx, grab, &c);
    ohc_rx_feed(&rx, fk.out, fk.out_len);
    fk.out_len = 0;
    return c.n;
}

void hal_init(void) {}
uint32_t hal_clock_hz(void) { return fk.clock; }
uint32_t hal_millis(void) { return fk.millis; }
uint32_t hal_irq_off(void) { return 0; }
void hal_irq_restore(uint32_t s) { (void)s; }
void hal_reset(void) { fk.resets++; }

int hal_host_getc(void)
{
    return fk.in_pos < fk.in_len ? fk.in[fk.in_pos++] : -1;
}

void hal_host_putc(uint8_t c)
{
    if (fk.out_len < sizeof(fk.out))
        fk.out[fk.out_len++] = c;
}

void hal_host_flush(void) {}

void hal_release(void)
{
    memset(fk.pin, 0, sizeof(fk.pin));
    memset(fk.carrier_period, 0, sizeof(fk.carrier_period));
    fk.burst_armed = false;
    fk.capture_on = false;
}

void hal_gpio_out(uint8_t port, uint8_t pin, bool level)
{
    fk.pin[port][pin].mode = PIN_OUT;
    fk.pin[port][pin].level = level;
}

void hal_gpio_set(uint8_t port, uint8_t pin, bool level)
{
    fk.pin[port][pin].level = level;
}

void hal_gpio_in(uint8_t port, uint8_t pin, uint8_t flags)
{
    fk.pin[port][pin].mode = PIN_IN;
    fk.pin[port][pin].pull = flags & (OHC_CFG_F_PULL_UP | OHC_CFG_F_PULL_DOWN);
}

bool hal_gpio_get(uint8_t port, uint8_t pin)
{
    return fk.pin[port][pin].level;
}

void hal_carrier_setup(const cfg_entry *e) { hal_gpio_out(e->port, e->pin, false); }
void hal_carrier_start(const cfg_entry *e, uint32_t t) { fk.carrier_period[e->func->unit] = t; }
void hal_carrier_gate(const cfg_entry *e, bool on) { fk.pin[e->port][e->pin].gated = on; }

void hal_carrier_stop(const cfg_entry *e)
{
    fk.pin[e->port][e->pin].gated = false;
    fk.carrier_period[e->func->unit] = 0;
}

void hal_burst_start(uint32_t ticks)
{
    fk.burst_armed = true;
    fk.burst_ticks = ticks;
}

void hal_burst_stop(void) { fk.burst_armed = false; }

void hal_capture_setup(const cfg_entry *e) { hal_gpio_in(e->port, e->pin, 0); }
void hal_capture_enable(bool on) { fk.capture_on = on; }
uint32_t hal_capture_now(void) { return fk.now; }

void hal_uart_setup(uint8_t port, const cfg_entry *rx, const cfg_entry *tx)
{
    (void)port;
    hal_gpio_in(rx->port, rx->pin, 0);
    hal_gpio_in(tx->port, tx->pin, 0);
}

void hal_uart_enable(uint8_t port, bool on, uint32_t baud, uint8_t bits,
                     uint8_t parity, uint8_t stop)
{
    (void)bits, (void)parity, (void)stop;
    fk.uart_on[port] = on;
    fk.uart_baud[port] = baud;
}

bool hal_uart_putc(uint8_t port, uint8_t c)
{
    if (fk.uart_fifo_room == 0)
        return false;
    if (fk.uart_fifo_room > 0)
        fk.uart_fifo_room--;
    fk.uart_tx[port][fk.uart_tx_len[port]++] = c;
    return true;
}
