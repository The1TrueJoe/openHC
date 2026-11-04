/* SPDX-License-Identifier: MIT */
/*
 * TM4C1231D5PM HAL. Written against the TM4C123x datasheet; no TivaWare.
 * GPIO through the APB apertures (the AHB ones are unmapped until GPIOHBCTL).
 * Carriers are timer halves in PWM mode, gated by handing the pin between the
 * timer and a GPIO driven low; capture is a 32-bit timer half in edge-time
 * mode; bursts are timed by TIMER4 as one 32-bit one-shot.
 *
 * Nothing is inherited from the bootloader: its RUN gates off and resets
 * UART0 before it jumps, so the clock and the host link are set up here.
 */
#include "../common/pl011.h"
#include "hal.h"
#include "ring.h"

const part_desc *hal_part = &part_tm4c1231d5;

#define SYSCTL          0x400FE000u
#define RCC             REG(SYSCTL + 0x060u)
#define RCC2            REG(SYSCTL + 0x070u)
#define PLLSTAT         REG(SYSCTL + 0x168u)
#define RCGCTIMER       REG(SYSCTL + 0x604u)
#define RCGCGPIO        REG(SYSCTL + 0x608u)
#define RCGCUART        REG(SYSCTL + 0x618u)
#define RCGCWTIMER      REG(SYSCTL + 0x65Cu)
#define PRTIMER         REG(SYSCTL + 0xA04u)
#define PRGPIO          REG(SYSCTL + 0xA08u)
#define PRUART          REG(SYSCTL + 0xA18u)
#define PRWTIMER        REG(SYSCTL + 0xA5Cu)

static const uint32_t gpio_base[6] = {
    0x40004000u, 0x40005000u, 0x40006000u, 0x40007000u, 0x40024000u, 0x40025000u,
};
#define GPIO_DATA(p, m) REG(gpio_base[p] + ((uint32_t)(m) << 2))
#define GPIO_DIR(p)     REG(gpio_base[p] + 0x400u)
#define GPIO_AFSEL(p)   REG(gpio_base[p] + 0x420u)
#define GPIO_PUR(p)     REG(gpio_base[p] + 0x510u)
#define GPIO_PDR(p)     REG(gpio_base[p] + 0x514u)
#define GPIO_DEN(p)     REG(gpio_base[p] + 0x51Cu)
#define GPIO_LOCK(p)    REG(gpio_base[p] + 0x520u)
#define GPIO_CR(p)      REG(gpio_base[p] + 0x524u)
#define GPIO_AMSEL(p)   REG(gpio_base[p] + 0x528u)
#define GPIO_PCTL(p)    REG(gpio_base[p] + 0x52Cu)
#define GPIO_UNLOCK     0x4C4F434Bu

/* timers: unit = 2 * timer + half; 0..11 = T0..T5, 12..23 = WT0..WT5 */
#define T_CFG(b)        REG((b) + 0x000u)
#define T_MR(b, h)      REG((b) + 0x004u + 4u * (h))
#define T_CTL(b)        REG((b) + 0x00Cu)
#define T_IMR(b)        REG((b) + 0x018u)
#define T_ICR(b)        REG((b) + 0x024u)
#define T_ILR(b, h)     REG((b) + 0x028u + 4u * (h))
#define T_MATCH(b, h)   REG((b) + 0x030u + 4u * (h))
#define T_PR(b, h)      REG((b) + 0x038u + 4u * (h))
#define T_R(b, h)       REG((b) + 0x048u + 4u * (h))
#define T_V(b, h)       REG((b) + 0x050u + 4u * (h))
#define T_EN(h)         (1u << (8u * (h)))
#define T_EVENT(h, v)   ((uint32_t)(v) << (2u + 8u * (h)))  /* 0 rising, 1 falling, 3 both */
#define T_TO(h)         (1u << (8u * (h)))                  /* timeout int */
#define T_CAE(h)        (1u << (2u + 8u * (h)))             /* capture event int */
#define MR_PERIODIC     0x2u
#define MR_ONESHOT      0x1u
#define MR_PWM          (0x2u | (1u << 3))                  /* periodic + alternate mode */
#define MR_EDGE_TIME_UP (0x3u | (1u << 2) | (1u << 4))      /* capture, edge-time, count up */

#define BURST           0x40034000u     /* TIMER4, concatenated */
#define BURST_IRQ       70u

static const uint8_t timer_irq_a[12] = { 19, 21, 23, 35, 70, 92, 94, 96, 98, 100, 102, 104 };

static uint32_t timer_base(uint8_t unit)
{
    uint8_t t = unit >> 1;

    if (t < 6)
        return 0x40030000u + 0x1000u * t;
    if (t < 8)
        return 0x40036000u + 0x1000u * (t - 6u);
    return 0x4004C000u + 0x1000u * (t - 8u);
}

/* timer B's interrupt is always the one after timer A's */
static uint32_t timer_irq(uint8_t unit)
{
    return timer_irq_a[unit >> 1] + (unit & 1u);
}

static void timer_clock(uint8_t unit)
{
    uint8_t t = unit >> 1;

    if (t < 6) {
        RCGCTIMER |= 1u << t;
        while (!(PRTIMER & (1u << t)))
            ;
    } else {
        RCGCWTIMER |= 1u << (t - 6u);
        while (!(PRWTIMER & (1u << (t - 6u))))
            ;
    }
}

static uint32_t clk = 50000000u;
static volatile uint32_t ms;
static ring host_rx;
static uint8_t host_rx_buf[512];
static uint8_t used[6];             /* pins we configured, per port */
static uint8_t timers_used;         /* timers (not halves) we started */

static void burst_init(void);

/* ---- core ---- */

#define UART0 0x4000C000u
#define UART_CC(b) REG((b) + 0xFC8u)

/* RCC2 for 80 MHz: PIOSC -> PLL (400 MHz) / 5, the clock stock and its bootloader run at */
#define RCC2_USERCC2    (1u << 31)
#define RCC2_DIV400     (1u << 30)
#define RCC2_SYSDIV_M   (0x7Fu << 22)   /* {SYSDIV2, SYSDIV2LSB} with DIV400 */
#define RCC2_SYSDIV_80  (4u << 22)
#define RCC2_PWRDN2     (1u << 13)
#define RCC2_BYPASS2    (1u << 11)
#define RCC2_OSCSRC_M   (7u << 4)
#define RCC2_PIOSC      (1u << 4)
#define RCC_XTAL_M      (0x1Fu << 6)
#define RCC_XTAL_16MHZ  (0x15u << 6)    /* the PLL's input is 16 MHz either way */
#define RCC_USESYSDIV   (1u << 22)
#define CLK_HZ          80000000u

static void clock_init(void)
{
    uint32_t want = RCC2_USERCC2 | RCC2_DIV400 | RCC2_SYSDIV_80 | RCC2_PIOSC;
    uint32_t mask = RCC2_USERCC2 | RCC2_DIV400 | RCC2_SYSDIV_M | RCC2_PWRDN2 |
                    RCC2_BYPASS2 | RCC2_OSCSRC_M;

    if ((RCC2 & mask) == want && (PLLSTAT & 1u))
        return;                 /* the bootloader left it there */
    RCC2 |= RCC2_USERCC2 | RCC2_BYPASS2;
    RCC = (RCC & ~RCC_XTAL_M) | RCC_XTAL_16MHZ | RCC_USESYSDIV;
    RCC2 = (RCC2 & ~(RCC2_OSCSRC_M | RCC2_PWRDN2 | RCC2_SYSDIV_M)) |
           RCC2_PIOSC | RCC2_DIV400 | RCC2_SYSDIV_80;
    /* PLLSTAT, not the lock interrupt: a PLL already locked never raises it again */
    for (uint32_t i = 0; i < 1000000u && !(PLLSTAT & 1u); i++)
        ;
    RCC2 &= ~RCC2_BYPASS2;
}

void hal_init(void)
{
    SCB_VTOR = 0x00001000u;     /* the bootloader leaves VTOR at its own table */
    clock_init();
    clk = CLK_HZ;

    SYSTICK_LOAD = clk / 1000u - 1u;
    SYSTICK_VAL = 0;
    SYSTICK_CTRL = 7;           /* core clock, interrupt, enable */

    /* UART0 on PA0/PA1: the bootloader's RUN left it gated off and in reset */
    RCGCUART |= 1u;
    while (!(PRUART & 1u))
        ;
    RCGCGPIO |= 1u;
    while (!(PRGPIO & 1u))
        ;
    GPIO_AMSEL(0) &= ~3u;
    GPIO_PCTL(0) = (GPIO_PCTL(0) & ~0xFFu) | 0x11u;
    GPIO_AFSEL(0) |= 3u;
    GPIO_DEN(0) |= 3u;
    UART_CC(UART0) = 0;         /* system clock */
    ring_init(&host_rx, host_rx_buf, sizeof(host_rx_buf));
    pl011_setup(UART0, clk, hal_part->host_baud, 8, 0, 1);
    nvic_enable(5, PRIO_UART);
    burst_init();
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

static void port_clock(uint8_t port)
{
    RCGCGPIO |= 1u << port;
    while (!(PRGPIO & (1u << port)))
        ;
}

static void pin_take(uint8_t port, uint8_t pin)
{
    uint8_t bit = (uint8_t)(1u << pin);

    port_clock(port);
    /* PF0 and PD7 are locked (NMI); unlock to change them */
    if ((port == 5 && pin == 0) || (port == 3 && pin == 7)) {
        GPIO_LOCK(port) = GPIO_UNLOCK;
        GPIO_CR(port) |= bit;
        GPIO_LOCK(port) = 0;
    }
    GPIO_AMSEL(port) &= ~(uint32_t)bit;
    used[port] |= bit;
}

static void pin_mux(uint8_t port, uint8_t pin, uint8_t mux)
{
    uint32_t sh = 4u * pin;

    GPIO_PCTL(port) = (GPIO_PCTL(port) & ~(0xfu << sh)) | ((uint32_t)mux << sh);
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

/* ---- user UARTs ---- */

#define UART_BASE(n) (UART0 + 0x1000u * (n))
static const uint8_t uart_irq[8] = { 5, 6, 33, 59, 60, 61, 62, 63 };

static struct {
    const cfg_entry *rx, *tx;
} uport[HAL_USER_UARTS];
static int8_t port_of_uart[8] = { -1, -1, -1, -1, -1, -1, -1, -1 };

void hal_uart_setup(uint8_t port, const cfg_entry *rx, const cfg_entry *tx)
{
    uint8_t n = rx->func->unit;

    uport[port].rx = rx;
    uport[port].tx = tx;
    port_of_uart[n] = (int8_t)port;
    RCGCUART |= 1u << n;
    while (!(PRUART & (1u << n)))
        ;
    hal_gpio_in(rx->port, rx->pin, 0);  /* idle as inputs until enabled */
    hal_gpio_in(tx->port, tx->pin, 0);
}

static void uart_pins(const cfg_entry *e, bool on)
{
    uint8_t bit = (uint8_t)(1u << e->pin);

    pin_mux(e->port, e->pin, on ? e->func->mux : 0);
    if (on)
        GPIO_AFSEL(e->port) |= bit;
    else
        GPIO_AFSEL(e->port) &= ~(uint32_t)bit;
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
        uart_pins(rx, true);
        uart_pins(tx, true);
        nvic_enable(uart_irq[n], PRIO_UART);
    } else {
        nvic_disable(uart_irq[n]);
        UART_IM(UART_BASE(n)) = 0;
        UART_CTL(UART_BASE(n)) = 0;
        uart_pins(rx, false);
        uart_pins(tx, false);
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
void uart3_isr(void) { user_uart_isr(3); }
void uart4_isr(void) { user_uart_isr(4); }
void uart5_isr(void) { user_uart_isr(5); }
void uart6_isr(void) { user_uart_isr(6); }
void uart7_isr(void) { user_uart_isr(7); }

/* ---- IR out ---- */

static void timer_split(uint8_t unit)
{
    uint32_t b = timer_base(unit);

    timer_clock(unit);
    if (!(timers_used & (1u << (unit >> 1)))) {
        T_CTL(b) = 0;
        T_CFG(b) = 0x4;         /* two halves; 32-bit each on a wide timer */
        timers_used |= (uint8_t)(1u << (unit >> 1));
    }
}

void hal_carrier_setup(const cfg_entry *e)
{
    timer_split(e->func->unit);
    hal_gpio_out(e->port, e->pin, false);
    pin_mux(e->port, e->pin, e->func->mux);
}

void hal_carrier_start(const cfg_entry *e, uint32_t period)
{
    uint8_t u = e->func->unit, h = u & 1u;
    uint32_t b = timer_base(u);

    if (period > 0xffffu)
        period = 0xffffu;
    T_CTL(b) &= ~T_EN(h);
    T_MR(b, h) = MR_PWM;
    T_PR(b, h) = 0;
    T_ILR(b, h) = period - 1u;
    /* high from reload down to the match: a third of the period */
    T_MATCH(b, h) = (period - 1u) - period / 3u;
    T_CTL(b) |= T_EN(h);
}

void hal_carrier_gate(const cfg_entry *e, bool on)
{
    uint8_t bit = (uint8_t)(1u << e->pin);

    if (on)
        GPIO_AFSEL(e->port) |= bit;
    else
        GPIO_AFSEL(e->port) &= ~(uint32_t)bit;  /* back to the GPIO, which is low */
}

void hal_carrier_stop(const cfg_entry *e)
{
    uint8_t u = e->func->unit;

    hal_carrier_gate(e, false);
    T_CTL(timer_base(u)) &= ~T_EN(u & 1u);
}

void hal_burst_start(uint32_t ticks)
{
    T_CTL(BURST) = 0;
    T_ILR(BURST, 0) = ticks;
    T_ICR(BURST) = T_TO(0);
    T_CTL(BURST) = T_EN(0);
}

void hal_burst_stop(void)
{
    T_CTL(BURST) = 0;
    T_ICR(BURST) = T_TO(0);
}

void burst_isr(void)
{
    T_ICR(BURST) = T_TO(0);
    app_burst_isr();
}

static void burst_init(void)
{
    RCGCTIMER |= 1u << 4;
    while (!(PRTIMER & (1u << 4)))
        ;
    T_CTL(BURST) = 0;
    T_CFG(BURST) = 0;           /* concatenated: one 32-bit timer */
    T_MR(BURST, 0) = MR_ONESHOT;
    T_IMR(BURST) = T_TO(0);
    nvic_enable(BURST_IRQ, PRIO_BURST);
}

/* ---- IR in ---- */

static const cfg_entry *cap;

void hal_capture_setup(const cfg_entry *e)
{
    uint8_t u = e->func->unit, h = u & 1u;
    uint32_t b = timer_base(u);

    cap = e;
    timer_split(u);
    hal_gpio_in(e->port, e->pin, 0);
    pin_mux(e->port, e->pin, e->func->mux);
    GPIO_AFSEL(e->port) |= 1u << e->pin;
    T_MR(b, h) = MR_EDGE_TIME_UP;
    T_ILR(b, h) = 0xffffffffu;
    T_CTL(b) = (T_CTL(b) & ~T_EVENT(h, 3)) |
               T_EVENT(h, (e->flags & OHC_CFG_F_DEMODULATED) ? 3 : 1);
    T_IMR(b) |= T_CAE(h);
}

void hal_capture_enable(bool on)
{
    uint8_t u, h;
    uint32_t b;

    if (!cap)
        return;
    u = cap->func->unit;
    h = u & 1u;
    b = timer_base(u);
    if (on) {
        T_ICR(b) = T_CAE(h);
        T_CTL(b) |= T_EN(h);
        nvic_enable(timer_irq(u), PRIO_CAPTURE);
    } else {
        nvic_disable(timer_irq(u));
        T_CTL(b) &= ~T_EN(h);
    }
}

uint32_t hal_capture_now(void)
{
    return cap ? T_V(timer_base(cap->func->unit), cap->func->unit & 1u) : 0;
}

void capture_isr(void)
{
    uint8_t u, h;
    uint32_t b, t;

    if (!cap)
        return;
    u = cap->func->unit;
    h = u & 1u;
    b = timer_base(u);
    T_ICR(b) = T_CAE(h);
    t = T_R(b, h);
    app_capture_edge(t, hal_gpio_get(cap->port, cap->pin));
}

/* ---- release ---- */

void hal_release(void)
{
    hal_burst_stop();
    if (cap) {
        hal_capture_enable(false);
        T_IMR(timer_base(cap->func->unit)) = 0;
        cap = 0;
    }
    for (uint8_t t = 0; t < 12; t++) {
        if (timers_used & (1u << t)) {
            T_CTL(timer_base((uint8_t)(2u * t))) = 0;
            T_IMR(timer_base((uint8_t)(2u * t))) = 0;
        }
    }
    timers_used = 0;
    for (uint8_t i = 0; i < HAL_USER_UARTS; i++)
        uport[i].rx = uport[i].tx = 0;
    for (uint8_t n = 0; n < 8; n++)
        port_of_uart[n] = -1;
    for (uint8_t p = 0; p < 6; p++) {
        uint8_t m = used[p];

        if (!m)
            continue;
        GPIO_AFSEL(p) &= ~(uint32_t)m;
        GPIO_DIR(p) &= ~(uint32_t)m;
        GPIO_PUR(p) &= ~(uint32_t)m;
        GPIO_PDR(p) &= ~(uint32_t)m;
        GPIO_DEN(p) &= ~(uint32_t)m;
        for (uint8_t pin = 0; pin < 8; pin++)
            if (m & (1u << pin))
                pin_mux(p, pin, 0);
        used[p] = 0;
    }
}
