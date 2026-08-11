/* TM4C1231D5 hardware layer: clock, UART, timing.
 *
 * Written against the datasheet; no TivaWare, no vendor code.
 */
#include "tm4c.h"

#include "ir_pins.h"

void tm4c_clock_init(void)
{
    /* 16 MHz crystal -> PLL (400 MHz) -> /8 = 50 MHz.
     * Matches OHC_SYSCLK_HZ, which the host-side capture decoder assumes when
     * turning reported carrier periods into Hz. */
    SYSCTL_RCC2 |= RCC2_USERCC2;
    SYSCTL_RCC2 |= RCC2_BYPASS2;                 /* run off the raw source while we fiddle */

    SYSCTL_RCC = (SYSCTL_RCC & ~(0x1Fu << RCC_XTAL_S)) | RCC_XTAL_16MHZ;
    SYSCTL_RCC &= ~RCC_MOSCDIS;                  /* main oscillator on */

    SYSCTL_RCC2 &= ~(0x7u << RCC2_OSCSRC2_S);    /* OSCSRC2 = 0 -> MOSC */
    SYSCTL_RCC2 &= ~RCC2_PWRDN2;                 /* power up the PLL */
    SYSCTL_RCC2 |= RCC2_DIV400;
    SYSCTL_RCC2 = (SYSCTL_RCC2 & ~RCC2_SYSDIV2_M) | (7u << RCC2_SYSDIV2_S);

    /* Clear a stale lock-interrupt first, so re-entering this function cannot
     * see the previous lock and continue before the PLL has actually settled. */
    SYSCTL_MISC = MISC_PLLLMIS;
    while (!(SYSCTL_RIS & RIS_PLLLRIS)) {        /* wait for PLL lock */
    }
    SYSCTL_RCC2 &= ~RCC2_BYPASS2;                /* switch onto the PLL */
}

void tm4c_uart_init(uint32_t base, uint32_t baud)
{
    /* Caller is responsible for clock-gating the UART and its GPIO pins; this
     * only programs the UART itself so it can be reused for all three ports. */
    UART_CTL(base) = 0;                          /* disable while configuring */

    /* Integer/fractional divisor: BRD = SYSCLK / (16 * baud), fraction in 1/64.
     * At 50 MHz / 460800 this is 6.7845 -> IBRD 6, FBRD 50, giving 460829 baud,
     * a 0.006% error and far inside the 2% a UART tolerates. */
    uint32_t brd = (OHC_SYSCLK_HZ * 4u) / baud;  /* = SYSCLK/(16*baud) in 1/64ths */
    UART_IBRD(base) = brd / 64u;
    UART_FBRD(base) = brd % 64u;

    UART_LCRH(base) = UART_LCRH_WLEN8 | UART_LCRH_FEN;   /* 8N1, FIFOs on */
    UART_CC(base) = 0;                                   /* system clock */
    UART_CTL(base) = UART_CTL_UARTEN | UART_CTL_TXE | UART_CTL_RXE;
}

void tm4c_uart_putc(uint32_t base, uint8_t c)
{
    while (UART_FR(base) & UART_FR_TXFF) {
    }
    UART_DR(base) = c;
}

int tm4c_uart_getc(uint32_t base)
{
    if (UART_FR(base) & UART_FR_RXFE) {
        return -1;
    }
    return (int)(UART_DR(base) & 0xFF);
}

void tm4c_delay_us(uint32_t us)
{
    /* Busy-wait on SysTick's 24-bit down-counter. Only used during bring-up;
     * the IR path is interrupt/timer driven and must not rely on this. */
    while (us--) {
        SYSTICK_LOAD = (OHC_SYSCLK_HZ / 1000000u) - 1u;
        SYSTICK_VAL = 0;
        SYSTICK_CTRL = 5;                        /* enable, core clock, no int */
        while (!(SYSTICK_CTRL & (1u << 16))) {
        }
        SYSTICK_CTRL = 0;
    }
}

/* ── clock gating helpers ─────────────────────────────────────────────────
 * Peripherals are unclocked out of reset; touching their registers before the
 * RCGC bit is set reads zero and writes vanish, with no fault to notice.
 */
static uint32_t gpio_rcgc_bit(uint32_t base)
{
    switch (base) {
    case GPIOA_BASE: return 1u << 0;
    case GPIOB_BASE: return 1u << 1;
    case GPIOC_BASE: return 1u << 2;
    case GPIOD_BASE: return 1u << 3;
    case GPIOE_BASE: return 1u << 4;
    case GPIOF_BASE: return 1u << 5;
    default:         return 0u;
    }
}

/* Wide timers are NOT in RCGCTIMER — they have their own RCGCWTIMER register.
 * Six of the ten IR channels use wide timers, so getting this wrong leaves most
 * of them silently dead. */
static void timer_clock_enable(uint32_t base)
{
    switch (base) {
    case TIMER0_BASE:  SYSCTL_RCGCTIMER  |= 1u << 0; break;
    case TIMER1_BASE:  SYSCTL_RCGCTIMER  |= 1u << 1; break;
    case TIMER2_BASE:  SYSCTL_RCGCTIMER  |= 1u << 2; break;
    case TIMER3_BASE:  SYSCTL_RCGCTIMER  |= 1u << 3; break;
    case WTIMER0_BASE: SYSCTL_RCGCWTIMER |= 1u << 0; break;
    case WTIMER1_BASE: SYSCTL_RCGCWTIMER |= 1u << 1; break;
    case WTIMER2_BASE: SYSCTL_RCGCWTIMER |= 1u << 2; break;
    case WTIMER3_BASE: SYSCTL_RCGCWTIMER |= 1u << 3; break;
    case WTIMER4_BASE: SYSCTL_RCGCWTIMER |= 1u << 4; break;
    case WTIMER5_BASE: SYSCTL_RCGCWTIMER |= 1u << 5; break;
    default: break;
    }
}

/* ── IR carrier ───────────────────────────────────────────────────────────
 *
 * TIMER0A in 16-bit PWM mode drives the carrier pin. `carrier_ticks` is the
 * full period in system-clock ticks; the match register sets the duty cycle.
 *
 * A ~33% duty is deliberate rather than 50%: IR receivers demodulate on carrier
 * presence, and a shorter on-time drives the same detection with roughly a
 * third less LED current, which matters when several emitters share the 5 V rail.
 *
 * UNTESTED ON HARDWARE — no board in hand at the time of writing. The register
 * sequence follows the TM4C123x datasheet's PWM-mode setup order (disable,
 * configure, load, enable), which is order-sensitive: programming TAMR while
 * the timer is enabled is silently ignored.
 */
void ir_carrier_configure(uint8_t channel, uint32_t carrier_ticks)
{
    if (channel >= IR_CHANNEL_COUNT) {
        return;
    }
    const ir_channel *ch = &IR_CHANNELS[channel];

    /* Clock-gate the port and its timer. WTIMER0/1 sit in RCGCTIMER's wide
     * block; the four narrow timers are bits 0..3. */
    SYSCTL_RCGCGPIO |= gpio_rcgc_bit(ch->gpio_base);
    timer_clock_enable(ch->timer_base);
    /* A few cycles must pass before the peripheral's registers are writable. */
    for (volatile int i = 0; i < 8; i++) {
    }

    if (carrier_ticks < 2u) {
        carrier_ticks = 2u;
    }
    if (carrier_ticks > 0xFFFFu) {
        carrier_ticks = 0xFFFFu;   /* 16-bit timer; below 763 Hz at 50 MHz */
    }

    uint32_t tb = ch->timer_base;
    TIMER_CTL(tb) &= ~TIMER_CTL_TAEN;                  /* must be off to configure */
    TIMER_CFG(tb) = TIMER_CFG_16BIT;
    TIMER_TAMR(tb) = TIMER_TAMR_PERIODIC | TIMER_TAMR_TAAMS;
    TIMER_TAILR(tb) = carrier_ticks - 1u;
    TIMER_TAMATCHR(tb) = (carrier_ticks / 3u);
    TIMER_CTL(tb) |= TIMER_CTL_TAEN;   /* runs continuously; the PIN is gated */

    /* Point the pin's alternate function at the timer, but leave AFSEL off so
     * nothing is emitted until ir_carrier_set(). */
    uint32_t gb = ch->gpio_base;
    int shift = 0;
    for (uint8_t m = ch->pin_mask; m > 1u; m >>= 1) {
        shift += 4;
    }
    GPIO_PCTL(gb) = (GPIO_PCTL(gb) & ~(0xFu << shift)) | (IR_PCTL_TIMER << shift);
    GPIO_DIR(gb) |= ch->pin_mask;
    GPIO_DEN(gb) |= ch->pin_mask;
    GPIO_AFSEL(gb) &= ~ch->pin_mask;                   /* start gated off */
    GPIO_DATA(gb) &= ~ch->pin_mask;
}

void ir_carrier_set(uint8_t channel, bool on)
{
    if (channel >= IR_CHANNEL_COUNT) {
        return;
    }
    const ir_channel *ch = &IR_CHANNELS[channel];
    /* Gate by handing the pin to the timer or back to plain GPIO (driven low),
     * rather than by stopping the timer. The oscillator keeps running, so every
     * mark starts on a clean carrier edge instead of wherever the timer happened
     * to restart — a restarting carrier smears the first cycle of each mark,
     * which is exactly what IR receivers use to detect burst onset.
     */
    if (on) {
        GPIO_AFSEL(ch->gpio_base) |= ch->pin_mask;
    } else {
        GPIO_AFSEL(ch->gpio_base) &= ~ch->pin_mask;
        GPIO_DATA(ch->gpio_base) &= ~ch->pin_mask;
    }
}

/* ── burst timing ─────────────────────────────────────────────────────────
 *
 * TIMER1A counts down one burst. 32-bit concatenated mode is mandatory, not a
 * preference: the worst case is a full-length burst at the slowest supported
 * carrier — 16383 carrier periods x 2500 ticks = 40,957,500 ticks (819 ms) —
 * which overflows a 16-bit timer and even a 16+8 prescaled one.
 *
 * Polled rather than interrupt-driven: during a transmission this is the only
 * thing happening, and polling keeps interrupt latency out of the burst edges.
 *
 * NOTE: TIMER1 is also IR channel... nothing. Channel 3 uses TIMER1 for its
 * carrier, so a transmission on channel 3 would fight this timer. Left as-is
 * deliberately for now — see README; the burst timer should move to a timer no
 * channel claims once the EA1's real channel set is known.
 */
void ir_burst_timer_start(uint32_t ticks)
{
    SYSCTL_RCGCTIMER |= (1u << 1);
    for (volatile int i = 0; i < 8; i++) {
    }
    if (ticks == 0u) {
        ticks = 1u;
    }
    TIMER_CTL(TIMER1_BASE) &= ~TIMER_CTL_TAEN;
    TIMER_CFG(TIMER1_BASE) = TIMER_CFG_32BIT;
    TIMER_TAMR(TIMER1_BASE) = TIMER_TAMR_PERIODIC;
    TIMER_TAILR(TIMER1_BASE) = ticks;
    TIMER_ICR(TIMER1_BASE) = TIMER_ICR_TATOCINT;
    TIMER_CTL(TIMER1_BASE) |= TIMER_CTL_TAEN;
}

bool ir_burst_timer_expired(void)
{
    if (TIMER_RIS(TIMER1_BASE) & TIMER_RIS_TATORIS) {
        TIMER_ICR(TIMER1_BASE) = TIMER_ICR_TATOCINT;
        return true;
    }
    return false;
}
