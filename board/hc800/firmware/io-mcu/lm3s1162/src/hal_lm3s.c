/* LM3S1162 hardware layer.
 *
 * Exposes exactly the surface the EA's TM4C HAL does, so src/main.c,
 * src/ir_engine.c and src/ohc_proto.c are the same code on both parts.
 */
#include "lm3s.h"

#include "ir_pins.h"

/* The system clock, DISCOVERED rather than configured — see lm3s_clock_init(). */
uint32_t g_sysclk_hz = OHC_SYSCLK_HZ;

/* Stellaris Fury has no PRGPIO/PRUART "peripheral ready" register to poll after
 * opening a clock gate (that is a Tiva addition). The datasheet's rule is to
 * allow a few clocks before touching the block; a read-back plus a short spin
 * is the portable way to spend them. */
void lm3s_clock_gate(volatile uint32_t *rcgc, uint32_t bit)
{
    *rcgc |= bit;
    (void)*rcgc;
    for (volatile int i = 0; i < 16; i++) {
    }
}

/* ── Clock ─────────────────────────────────────────────────────────────────
 *
 * We do NOT reprogram the PLL, and that is a deliberate choice with a reason
 * behind it rather than an omission.
 *
 * To set up the PLL we would have to write RCC's XTAL field, and the XTAL field
 * encodes the CRYSTAL FITTED TO THIS BOARD — which we do not know. Nothing in
 * the vendor image reveals it, no schematic exists, and a wrong XTAL does not
 * fail loudly: it yields a running part with a silently wrong clock, which
 * means an IR carrier at the wrong frequency and a host link at the wrong baud.
 *
 * But the clock is already correct when we get here. Control4's bootloader owns
 * flash 0x0000, brings the part up, and talks to the host over UART0 at 115200
 * — a rate we have MEASURED against a live unit. So rather than reconfigure,
 * we measure what the bootloader left us:
 *
 *     baud = clk / (16 * (IBRD + FBRD/64))
 *     =>  clk = baud * 16 * (IBRD + FBRD/64)
 *
 * Reading the divisors the bootloader programmed, and knowing the baud those
 * divisors produce, gives the system clock with no guess anywhere in the chain.
 * Everything downstream — the IR carrier period, the burst timer, delays — is
 * then expressed against a number that is true rather than assumed.
 *
 * If the divisors read back as nonsense we keep the OHC_SYSCLK_HZ default and
 * carry on, because a plausible clock beats a division by zero.
 */
void lm3s_clock_init(void)
{
    uint32_t ibrd = UART_IBRD(UART_HOST_BASE) & 0xFFFFu;
    uint32_t fbrd = UART_FBRD(UART_HOST_BASE) & 0x3Fu;

    if (ibrd != 0u) {
        /* clk = baud * (16*IBRD + FBRD/4), all integer: 16*(IBRD + FBRD/64)
         * is 16*IBRD + FBRD/4. */
        g_sysclk_hz = OHC_HOST_BAUD * (16u * ibrd + (fbrd + 2u) / 4u);
    }

    /* Timer for burst timing, and the PWM module for the carriers. */
    lm3s_clock_gate(&SYSCTL_RCGC1, RCGC1_TIMER(IR_BURST_TIMER_RCGC));
    lm3s_clock_gate(&SYSCTL_RCGC0, RCGC0_PWM);

    /* PWM runs straight off the system clock: USEPWMDIV off means the module
     * clock IS the system clock, so carrier_ticks from ir_engine needs no
     * second scaling factor. One less place to be wrong. */
    SYSCTL_RCC &= ~RCC_USEPWMDIV;
}

/* ── UART (PL011) ───────────────────────────────────────────────────────────
 *
 * NOTE what this does not do: it does not touch the GPIO mux. The bootloader
 * has already muxed UART0's pins — it is talking to the host over them — and
 * since this firmware does not know which pins those are (see ir_pins.h),
 * inheriting a working configuration is both safer and one fewer unknown.
 */
void lm3s_uart_init(uint32_t base, uint32_t baud)
{
    uint32_t div;

    UART_CTL(base) = 0u;

    /* 16*baud fixed-point divisor, rounded. */
    div = (g_sysclk_hz * 4u + baud / 2u) / baud;
    UART_IBRD(base) = div / 64u;
    UART_FBRD(base) = div % 64u;

    UART_LCRH(base) = UART_LCRH_WLEN8 | UART_LCRH_FEN;   /* 8N1, FIFOs on */
    UART_CTL(base)  = UART_CTL_UARTEN | UART_CTL_TXE | UART_CTL_RXE;
}

void lm3s_uart_putc(uint32_t base, uint8_t c)
{
    while (UART_FR(base) & UART_FR_TXFF) {
    }
    UART_DR(base) = c;
}

int lm3s_uart_getc(uint32_t base)
{
    if (UART_FR(base) & UART_FR_RXFE) {
        return -1;
    }
    return (int)(UART_DR(base) & 0xFFu);
}

void lm3s_delay_us(uint32_t us)
{
    /* Three cycles per iteration on Cortex-M3 for this loop shape. Only used
     * for coarse settling, never for IR timing — that is the GPTM's job. */
    uint32_t iters = (g_sysclk_hz / 3000000u) * us;
    while (iters--) {
        __asm volatile("nop");
    }
}

/* ── IR carrier, on the PWM module ─────────────────────────────────────────
 *
 * One generator drives TWO outputs from ONE counter, so channels that share a
 * generator (0+1, 2+3, 4+5) necessarily share a carrier frequency. That costs
 * nothing in practice: IROUT_SEND carries a single carrier for the whole
 * output_mask, so every channel in one transmission wants the same period
 * anyway.
 *
 * Duty is 1/3, the conventional IR figure — it puts more energy per burst
 * through the emitter than 50% for the same average current.
 */
void ir_carrier_configure(uint8_t channel, uint32_t carrier_ticks)
{
    const ir_channel *c;
    uint32_t load, duty;

    if (channel >= IR_CHANNEL_COUNT) {
        return;
    }
    c = &IR_CHANNELS[channel];

    /* PWM counters are 16-bit. A carrier slower than clk/65536 (763 Hz at
     * 50 MHz) cannot be represented — far below the protocol's 20 kHz floor,
     * so clamping here is a guard, not a limitation. */
    load = carrier_ticks ? carrier_ticks - 1u : 1u;
    if (load > 0xFFFFu) {
        load = 0xFFFFu;
    }
    duty = load / 3u;

    PWMn_CTL(c->pwm_gen)  = 0u;                 /* stop while reprogramming */
    PWMn_LOAD(c->pwm_gen) = load;
    if (c->pwm_out == 0u) {
        PWMn_CMPA(c->pwm_gen) = duty;
        PWMn_GENA(c->pwm_gen) = PWM_GENA_LOAD_HIGH | PWM_GENA_CMPAD_LOW;
    } else {
        PWMn_CMPB(c->pwm_gen) = duty;
        PWMn_GENB(c->pwm_gen) = PWM_GENB_LOAD_HIGH | PWM_GENB_CMPBD_LOW;
    }
    PWMn_CTL(c->pwm_gen) = PWMn_CTL_ENABLE;

#if OHC_IR_PINS_KNOWN
    /* Route the generator output to its pin. Gated because the pin map is not
     * decoded yet — see the header of ir_pins.h. Without this the carrier runs
     * inside the peripheral and reaches nothing, which is exactly what we want
     * on a unit that may be wired to real equipment. */
    lm3s_clock_gate(&SYSCTL_RCGC2, RCGC2_GPIO(gpio_port_index(c->gpio_base)));
    GPIO_AFSEL(c->gpio_base) |= c->pin_mask;
    GPIO_DEN(c->gpio_base)   |= c->pin_mask;
    GPIO_DR8R(c->gpio_base)  |= c->pin_mask;   /* IR LEDs want the 8 mA driver */
#endif
}

void ir_carrier_set(uint8_t channel, bool on)
{
    uint32_t bit;

    if (channel >= IR_CHANNEL_COUNT) {
        return;
    }
    bit = IR_PWM_ENABLE_BIT(channel);

    /* PWMENABLE gates the output stage, so gating there rather than stopping
     * the generator keeps the carrier phase-continuous across a burst. */
    if (on) {
        PWM_ENABLE |= bit;
    } else {
        PWM_ENABLE &= ~bit;
    }
}

/* ── Burst timing, on a 32-bit GPTM ───────────────────────────────────────── */
void ir_burst_timer_start(uint32_t ticks)
{
    uint32_t base = IR_BURST_TIMER_BASE;

    if (ticks == 0u) {
        ticks = 1u;
    }
    GPTM_CTL(base)   = 0u;
    GPTM_CFG(base)   = GPTM_CFG_32BIT;
    GPTM_TAMR(base)  = GPTM_TAMR_ONESHOT;
    GPTM_TAILR(base) = ticks;
    GPTM_ICR(base)   = GPTM_RIS_TATORIS;      /* clear a stale time-out */
    GPTM_CTL(base)   = GPTM_CTL_TAEN;
}

bool ir_burst_timer_expired(void)
{
    return (GPTM_RIS(IR_BURST_TIMER_BASE) & GPTM_RIS_TATORIS) != 0u;
}
