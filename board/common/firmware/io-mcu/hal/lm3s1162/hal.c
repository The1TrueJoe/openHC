/* SPDX-License-Identifier: MIT */
/*
 * LM3S1162 HAL (Stellaris Fury). Clock gating is RCGC0/1/2 with no ready
 * register; there is no port control, AFSEL alone picks a pin's one
 * alternate function. Carriers come from the PWM module, gated like the
 * TM4C's by handing the pin between PWM and a GPIO driven low. Bursts are
 * timed by TIMER0 as a 32-bit one-shot; the receiver is a GPIO edge interrupt
 * timestamped from TIMER1 free-running.
 */
#include "../common/pl011.h"
#include "hal.h"
#include "ring.h"

const part_desc *hal_part = &part_lm3s1162;

#define SYSCTL      0x400FE000u
#define RCC         REG(SYSCTL + 0x060u)
#define RCGC0       REG(SYSCTL + 0x100u)
#define RCGC1       REG(SYSCTL + 0x104u)
#define RCGC2       REG(SYSCTL + 0x108u)
#define RCGC0_PWM   (1u << 20)
#define RCC_USEPWMDIV (1u << 20)

static const uint32_t gpio_base[8] = {
    0x40004000u, 0x40005000u, 0x40006000u, 0x40007000u,
    0x40024000u, 0x40025000u, 0x40026000u, 0x40027000u,
};
static const uint8_t gpio_irq[8] = { 0, 1, 2, 3, 4, 30, 31, 32 };
#define GPIO_DATA(p, m) REG(gpio_base[p] + ((uint32_t)(m) << 2))
#define GPIO_DIR(p)     REG(gpio_base[p] + 0x400u)
#define GPIO_IS(p)      REG(gpio_base[p] + 0x404u)
#define GPIO_IBE(p)     REG(gpio_base[p] + 0x408u)
#define GPIO_IEV(p)     REG(gpio_base[p] + 0x40Cu)
#define GPIO_IM(p)      REG(gpio_base[p] + 0x410u)
#define GPIO_MIS(p)     REG(gpio_base[p] + 0x418u)
#define GPIO_ICR(p)     REG(gpio_base[p] + 0x41Cu)
#define GPIO_AFSEL(p)   REG(gpio_base[p] + 0x420u)
#define GPIO_DR8R(p)    REG(gpio_base[p] + 0x508u)
#define GPIO_PUR(p)     REG(gpio_base[p] + 0x510u)
#define GPIO_PDR(p)     REG(gpio_base[p] + 0x514u)
#define GPIO_DEN(p)     REG(gpio_base[p] + 0x51Cu)

#define PWM         0x40028000u
#define PWM_ENABLE  REG(PWM + 0x008u)
#define PWM_CTL(g)  REG(PWM + 0x040u + 0x40u * (g))
#define PWM_LOAD(g) REG(PWM + 0x050u + 0x40u * (g))
#define PWM_CMPA(g) REG(PWM + 0x058u + 0x40u * (g))
#define PWM_CMPB(g) REG(PWM + 0x05Cu + 0x40u * (g))
#define PWM_GENA(g) REG(PWM + 0x060u + 0x40u * (g))
#define PWM_GENB(g) REG(PWM + 0x064u + 0x40u * (g))
/* count-down: high at load, low at the compare going down */
#define GENA_SET    ((3u << 2) | (2u << 6))     /* ACTLOAD one, ACTCMPAD zero */
#define GENB_SET    ((3u << 2) | (2u << 10))    /* ACTLOAD one, ACTCMPBD zero */

#define T_CFG(b)    REG((b) + 0x000u)
#define T_TAMR(b)   REG((b) + 0x004u)
#define T_CTL(b)    REG((b) + 0x00Cu)
#define T_IMR(b)    REG((b) + 0x018u)
#define T_ICR(b)    REG((b) + 0x024u)
#define T_TAILR(b)  REG((b) + 0x028u)
#define T_TAR(b)    REG((b) + 0x048u)
#define BURST       0x40030000u     /* TIMER0 */
#define STAMP       0x40031000u     /* TIMER1 */
#define BURST_IRQ   19u

#define UART0       0x4000C000u
#define UART_BASE(n) (UART0 + 0x1000u * (n))
static const uint8_t uart_irq[3] = { 5, 6, 33 };

static uint32_t clk = 50000000u;
static volatile uint32_t ms;
static ring host_rx;
static uint8_t host_rx_buf[512];
static uint8_t used[8];

static void gate(volatile uint32_t *rcgc, uint32_t bit)
{
    *rcgc |= bit;
    (void)*rcgc;
    for (volatile int i = 0; i < 16; i++)
        ;   /* Fury has no ready register: a few clocks before the block answers */
}

/* ---- core ---- */

void hal_init(void)
{
    SCB_VTOR = 0x00001000u;
    clk = clock_from_divisor(hal_part->host_baud, UART_IBRD(UART0), UART_FBRD(UART0), clk);

    SYSTICK_LOAD = clk / 1000u - 1u;
    SYSTICK_VAL = 0;
    SYSTICK_CTRL = 7;

    ring_init(&host_rx, host_rx_buf, sizeof(host_rx_buf));
    pl011_take_over(UART0);
    nvic_enable(5, PRIO_UART);

    gate(&RCGC1, (1u << 16) | (1u << 17));  /* TIMER0, TIMER1 */
    T_CTL(BURST) = 0;
    T_CFG(BURST) = 0;
    T_TAMR(BURST) = 0x1;        /* one-shot */
    T_IMR(BURST) = 1;           /* timeout */
    nvic_enable(BURST_IRQ, PRIO_BURST);
    T_CTL(STAMP) = 0;
    T_CFG(STAMP) = 0;
    T_TAMR(STAMP) = 0x2;        /* periodic, counting down */
    T_TAILR(STAMP) = 0xffffffffu;
    T_CTL(STAMP) = 1;

    gate(&RCGC0, RCGC0_PWM);
    RCC &= ~RCC_USEPWMDIV;      /* PWM clock = system clock */
    __asm volatile("cpsie i");
}

uint32_t hal_clock_hz(void) { return clk; }
uint32_t hal_millis(void) { return ms; }
uint32_t hal_irq_off(void) { return irq_save(); }
void hal_irq_restore(uint32_t s) { irq_restore(s); }
void hal_reset(void) { sys_reset(); }

void systick_isr(void) { ms++; }

void uart0_isr(void)
{
    int c;

    while ((c = pl011_getc(UART0)) >= 0)
        ring_put(&host_rx, (uint8_t)c);
    pl011_ack(UART0);
}

int hal_host_getc(void) { return ring_get(&host_rx); }

void hal_host_putc(uint8_t c)
{
    while (!pl011_putc(UART0, c))
        ;
}

void hal_host_flush(void) { pl011_wait_idle(UART0); }

/* ---- GPIO ---- */

static void pin_take(uint8_t port, uint8_t pin)
{
    gate(&RCGC2, 1u << port);
    used[port] |= (uint8_t)(1u << pin);
}

void hal_gpio_out(uint8_t port, uint8_t pin, bool level)
{
    uint8_t bit = (uint8_t)(1u << pin);

    pin_take(port, pin);
    GPIO_AFSEL(port) &= ~(uint32_t)bit;
    GPIO_DATA(port, bit) = level ? bit : 0;
    GPIO_DIR(port) |= bit;
    GPIO_DEN(port) |= bit;
}

void hal_gpio_set(uint8_t port, uint8_t pin, bool level)
{
    uint8_t bit = (uint8_t)(1u << pin);

    GPIO_DATA(port, bit) = level ? bit : 0;
}

void hal_gpio_in(uint8_t port, uint8_t pin, uint8_t flags)
{
    uint8_t bit = (uint8_t)(1u << pin);

    pin_take(port, pin);
    GPIO_AFSEL(port) &= ~(uint32_t)bit;
    GPIO_DIR(port) &= ~(uint32_t)bit;
    if (flags & OHC_CFG_F_PULL_UP)
        GPIO_PUR(port) |= bit;
    if (flags & OHC_CFG_F_PULL_DOWN)
        GPIO_PDR(port) |= bit;
    GPIO_DEN(port) |= bit;
}

bool hal_gpio_get(uint8_t port, uint8_t pin)
{
    return GPIO_DATA(port, 1u << pin) != 0;
}

static void afsel(const cfg_entry *e, bool on)
{
    uint8_t bit = (uint8_t)(1u << e->pin);

    if (on)
        GPIO_AFSEL(e->port) |= bit;
    else
        GPIO_AFSEL(e->port) &= ~(uint32_t)bit;
}

/* ---- user UARTs (UART1, UART2) ---- */

static struct {
    const cfg_entry *rx, *tx;
} uport[HAL_USER_UARTS];
static int8_t port_of_uart[3] = { -1, -1, -1 };

void hal_uart_setup(uint8_t port, const cfg_entry *rx, const cfg_entry *tx)
{
    uint8_t n = rx->func->unit;

    uport[port].rx = rx;
    uport[port].tx = tx;
    port_of_uart[n] = (int8_t)port;
    gate(&RCGC1, 1u << n);
    hal_gpio_in(rx->port, rx->pin, 0);
    hal_gpio_in(tx->port, tx->pin, 0);
}

void hal_uart_enable(uint8_t port, bool on, uint32_t baud, uint8_t bits,
                     uint8_t parity, uint8_t stop)
{
    const cfg_entry *rx = uport[port].rx, *tx = uport[port].tx;
    uint8_t n;

    if (!rx)
        return;
    n = rx->func->unit;
    if (on) {
        pl011_setup(UART_BASE(n), clk, baud, bits, parity, stop);
        afsel(rx, true);
        afsel(tx, true);
        nvic_enable(uart_irq[n], PRIO_UART);
    } else {
        nvic_disable(uart_irq[n]);
        UART_IM(UART_BASE(n)) = 0;
        UART_CTL(UART_BASE(n)) = 0;
        afsel(rx, false);
        afsel(tx, false);
    }
}

bool hal_uart_putc(uint8_t port, uint8_t c)
{
    return uport[port].rx && pl011_putc(UART_BASE(uport[port].rx->func->unit), c);
}

static void user_uart_isr(uint8_t n)
{
    uint32_t b = UART_BASE(n);
    int c;

    while ((c = pl011_getc(b)) >= 0)
        if (port_of_uart[n] >= 0)
            app_uart_rx((uint8_t)port_of_uart[n], (uint8_t)c);
    pl011_ack(b);
}

void uart1_isr(void) { user_uart_isr(1); }
void uart2_isr(void) { user_uart_isr(2); }

/* ---- IR out ---- */

void hal_carrier_setup(const cfg_entry *e)
{
    hal_gpio_out(e->port, e->pin, false);
    GPIO_DR8R(e->port) |= 1u << e->pin;     /* IR LEDs want the 8 mA driver */
}

void hal_carrier_start(const cfg_entry *e, uint32_t period)
{
    uint8_t out = e->func->unit, g = out >> 1;
    uint32_t load;

    if (period > 0x10000u)
        period = 0x10000u;
    load = period - 1u;
    /* both outputs of a generator share its counter: same carrier */
    PWM_CTL(g) = 0;
    PWM_LOAD(g) = load;
    PWM_CMPA(g) = load - period / 3u;
    PWM_CMPB(g) = load - period / 3u;
    PWM_GENA(g) = GENA_SET;
    PWM_GENB(g) = GENB_SET;
    PWM_CTL(g) = 1;
    PWM_ENABLE |= 1u << out;
}

void hal_carrier_gate(const cfg_entry *e, bool on)
{
    afsel(e, on);
}

void hal_carrier_stop(const cfg_entry *e)
{
    uint8_t out = e->func->unit, g = out >> 1;

    afsel(e, false);
    PWM_ENABLE &= ~(1u << out);
    if (!(PWM_ENABLE & (3u << (2u * g))))
        PWM_CTL(g) = 0;
}

void hal_burst_start(uint32_t ticks)
{
    T_CTL(BURST) = 0;
    T_TAILR(BURST) = ticks;
    T_ICR(BURST) = 1;
    T_CTL(BURST) = 1;
}

void hal_burst_stop(void)
{
    T_CTL(BURST) = 0;
    T_ICR(BURST) = 1;
}

void burst_isr(void)
{
    T_ICR(BURST) = 1;
    app_burst_isr();
}

/* ---- IR in ---- */

static const cfg_entry *cap;

void hal_capture_setup(const cfg_entry *e)
{
    uint8_t bit = (uint8_t)(1u << e->pin);

    cap = e;
    hal_gpio_in(e->port, e->pin, 0);
    GPIO_IM(e->port) &= ~(uint32_t)bit;
    GPIO_IS(e->port) &= ~(uint32_t)bit;         /* edge */
    if (e->flags & OHC_CFG_F_DEMODULATED) {
        GPIO_IBE(e->port) |= bit;               /* both edges */
    } else {
        GPIO_IBE(e->port) &= ~(uint32_t)bit;
        GPIO_IEV(e->port) &= ~(uint32_t)bit;    /* falling, once per carrier cycle */
    }
    GPIO_ICR(e->port) = bit;
}

void hal_capture_enable(bool on)
{
    uint8_t bit;

    if (!cap)
        return;
    bit = (uint8_t)(1u << cap->pin);
    if (on) {
        GPIO_ICR(cap->port) = bit;
        GPIO_IM(cap->port) |= bit;
        nvic_enable(gpio_irq[cap->port], PRIO_CAPTURE);
    } else {
        GPIO_IM(cap->port) &= ~(uint32_t)bit;
        nvic_disable(gpio_irq[cap->port]);
    }
}

uint32_t hal_capture_now(void)
{
    return ~T_TAR(STAMP);       /* counting down: invert to count up */
}

void gpio_isr(void)
{
    uint32_t t = ~T_TAR(STAMP), mis;
    uint8_t bit;

    if (!cap)
        return;
    bit = (uint8_t)(1u << cap->pin);
    mis = GPIO_MIS(cap->port);
    GPIO_ICR(cap->port) = mis;
    if (mis & bit)
        app_capture_edge(t, hal_gpio_get(cap->port, cap->pin));
}

/* ---- release ---- */

void hal_release(void)
{
    hal_burst_stop();
    if (cap) {
        hal_capture_enable(false);
        cap = 0;
    }
    for (uint8_t g = 0; g < 3; g++)
        PWM_CTL(g) = 0;
    PWM_ENABLE = 0;
    for (uint8_t i = 0; i < HAL_USER_UARTS; i++)
        uport[i].rx = uport[i].tx = 0;
    for (uint8_t n = 0; n < 3; n++)
        port_of_uart[n] = -1;
    for (uint8_t p = 0; p < 8; p++) {
        uint8_t m = used[p];

        if (!m)
            continue;
        GPIO_IM(p) &= ~(uint32_t)m;
        GPIO_AFSEL(p) &= ~(uint32_t)m;
        GPIO_DIR(p) &= ~(uint32_t)m;
        GPIO_PUR(p) &= ~(uint32_t)m;
        GPIO_PDR(p) &= ~(uint32_t)m;
        GPIO_DR8R(p) &= ~(uint32_t)m;
        GPIO_DEN(p) &= ~(uint32_t)m;
        used[p] = 0;
    }
}
