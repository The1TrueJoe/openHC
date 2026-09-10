/* Register map for the TI Stellaris LM3S1162 (Cortex-M3) IO processor.
 *
 * NOT a copy of the TM4C header next door. The Stellaris "Fury" generation
 * differs from Tiva in three ways that matter here, and each one is a silent
 * failure if you assume Tiva:
 *
 *   1. CLOCK GATING IS RCGC0/1/2, not RCGCGPIO/RCGCUART/RCGCPWM. There is also
 *      no PRGPIO/PRUART "peripheral ready" register to poll — Fury needs a few
 *      cycles of delay after a gate is opened instead (see lm3s_clock_gate()).
 *   2. THERE IS NO GPIOPCTL. Port control / PMCn arrived with DustDevil and
 *      Tempest. On Fury an alternate function is selected by AFSEL alone, and
 *      which function you get is fixed by the pin. Writing a PCTL that does not
 *      exist is a no-op into reserved space — the pin silently stays GPIO.
 *   3. THE PART HAS A REAL PWM MODULE. The EA's TM4C1231D5 does not, which is
 *      why that firmware synthesises the IR carrier from a timer CCP output.
 *      Here PWM generators 0/1/2 do it in hardware.
 *
 * ── Where these addresses come from ───────────────────────────────────────
 *
 * The peripheral BASE addresses below are not datasheet transcription: every
 * one of them was found as a literal inside the stock image
 * (700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_..., scanned at
 * board/hc800/firmware/io-mcu/lm3s1162 development time) — GPIOA-H, UART0-2,
 * TIMER0-3, PWM at 0x40028000, ADC0 and SYSCTL all appear there, which is as
 * good a confirmation as a datasheet and better than a guess.
 *
 * The register OFFSETS within each block are the standard Stellaris peripheral
 * layout. They are shared with Tiva for GPIO and the PL011 UART, and the EA
 * firmware exercises those same offsets on real hardware. THE PWM OFFSETS ARE
 * THE ONE GROUP NOT YET EXERCISED BY ANYTHING WE HAVE RUN — check them against
 * the LM3S1162 datasheet before the first flash.
 */
#ifndef OHC_LM3S_H
#define OHC_LM3S_H

#include <stdbool.h>
#include <stdint.h>

#define REG32(a) (*(volatile uint32_t *)(uintptr_t)(a))

/* ── System control ───────────────────────────────────────────────────────── */
#define SYSCTL_BASE   0x400FE000u
#define SYSCTL_DID0   REG32(SYSCTL_BASE + 0x000u)
#define SYSCTL_DID1   REG32(SYSCTL_BASE + 0x004u)
/* DC0: bits 31:16 SRAMSZ, bits 15:0 FLASHSZ. Sizes are (field + 1) * 256 and
 * (field + 1) * 2048 bytes respectively. This is how startup.c learns the real
 * SRAM top without the linker script having to know it. */
#define SYSCTL_DC0    REG32(SYSCTL_BASE + 0x008u)
#define SYSCTL_RCC    REG32(SYSCTL_BASE + 0x060u)
#define SYSCTL_RCC2   REG32(SYSCTL_BASE + 0x070u)
#define SYSCTL_RIS    REG32(SYSCTL_BASE + 0x050u)
#define SYSCTL_RCGC0  REG32(SYSCTL_BASE + 0x100u)
#define SYSCTL_RCGC1  REG32(SYSCTL_BASE + 0x104u)
#define SYSCTL_RCGC2  REG32(SYSCTL_BASE + 0x108u)

#define SYSCTL_DC0_SRAMSZ(dc0)  ((((dc0) >> 16) & 0xFFFFu) + 1u)   /* * 256 B */
#define SYSCTL_DC0_FLASHSZ(dc0) ((((dc0) >>  0) & 0xFFFFu) + 1u)   /* * 2048 B */

/* RCGC0 */
#define RCGC0_PWM     (1u << 20)
/* RCGC1 — UARTs are bits 0..2 */
#define RCGC1_UART(n) (1u << (n))
/* RCGC2 — GPIO ports A..H are bits 0..7 */
#define RCGC2_GPIO(n) (1u << (n))

/* RCC / RCC2 fields used by lm3s_clock_init(). */
#define RCC_MOSCDIS   (1u << 0)
#define RCC_OSCSRC_M  (3u << 4)
#define RCC_XTAL_M    (0x1Fu << 6)
#define RCC_XTAL_S    6
#define RCC_BYPASS    (1u << 11)
#define RCC_PWRDN     (1u << 13)
#define RCC_USEPWMDIV (1u << 20)
#define RCC_PWMDIV_M  (7u << 17)
#define RCC_PWMDIV_S  17
#define RCC_USESYSDIV (1u << 22)
#define RCC_SYSDIV_M  (0xFu << 23)
#define RCC_SYSDIV_S  23
#define RCC2_USERCC2  (1u << 31)
#define RCC2_PWRDN2   (1u << 13)
#define RCC2_BYPASS2  (1u << 11)
#define RIS_PLLLRIS   (1u << 6)

/* ── GPIO (APB aperture) ──────────────────────────────────────────────────── */
#define GPIOA_BASE 0x40004000u
#define GPIOB_BASE 0x40005000u
#define GPIOC_BASE 0x40006000u
#define GPIOD_BASE 0x40007000u
#define GPIOE_BASE 0x40024000u
#define GPIOF_BASE 0x40025000u
#define GPIOG_BASE 0x40026000u
#define GPIOH_BASE 0x40027000u

/* Port index for RCGC2, derived from the base so callers pass one thing. */
static inline uint32_t gpio_port_index(uint32_t base)
{
    switch (base) {
    case GPIOA_BASE: return 0u;
    case GPIOB_BASE: return 1u;
    case GPIOC_BASE: return 2u;
    case GPIOD_BASE: return 3u;
    case GPIOE_BASE: return 4u;
    case GPIOF_BASE: return 5u;
    case GPIOG_BASE: return 6u;
    default:         return 7u;   /* GPIOH */
    }
}

/* GPIODATA is bit-masked: the address bits 9:2 ARE the pin mask, so a write
 * through GPIO_DATA(base, mask) touches only those pins with no read-modify-
 * write and no interrupt window. This is the whole reason the block is 4 KB. */
#define GPIO_DATA(base, mask) REG32((base) + ((uint32_t)(mask) << 2))
#define GPIO_DIR(base)   REG32((base) + 0x400u)
#define GPIO_AFSEL(base) REG32((base) + 0x420u)
#define GPIO_DR2R(base)  REG32((base) + 0x500u)
#define GPIO_DR8R(base)  REG32((base) + 0x508u)
#define GPIO_PUR(base)   REG32((base) + 0x510u)
#define GPIO_DEN(base)   REG32((base) + 0x51Cu)

/* ── UART (PL011, same layout as Tiva) ────────────────────────────────────── */
#define UART0_BASE 0x4000C000u
#define UART1_BASE 0x4000D000u
#define UART2_BASE 0x4000E000u

#define UART_DR(base)   REG32((base) + 0x000u)
#define UART_FR(base)   REG32((base) + 0x018u)
#define UART_IBRD(base) REG32((base) + 0x024u)
#define UART_FBRD(base) REG32((base) + 0x028u)
#define UART_LCRH(base) REG32((base) + 0x02Cu)
#define UART_CTL(base)  REG32((base) + 0x030u)

#define UART_FR_RXFE (1u << 4)   /* RX FIFO empty */
#define UART_FR_TXFF (1u << 5)   /* TX FIFO full  */
#define UART_LCRH_FEN  (1u << 4)
#define UART_LCRH_WLEN8 (3u << 5)
#define UART_CTL_UARTEN (1u << 0)
#define UART_CTL_TXE    (1u << 8)
#define UART_CTL_RXE    (1u << 9)

/* ── PWM — the IR carrier generator ───────────────────────────────────────── */
#define PWM_BASE 0x40028000u

#define PWM_CTL     REG32(PWM_BASE + 0x000u)
#define PWM_SYNC    REG32(PWM_BASE + 0x004u)
#define PWM_ENABLE  REG32(PWM_BASE + 0x008u)
#define PWM_INVERT  REG32(PWM_BASE + 0x00Cu)
#define PWM_FAULT   REG32(PWM_BASE + 0x010u)
#define PWM_INTEN   REG32(PWM_BASE + 0x014u)

/* Generators 0..3 at stride 0x40 from 0x040. Each drives two outputs:
 * generator n owns PWM(2n) and PWM(2n+1). Three generators = six outputs,
 * which is what makes six IR channels possible on this part. */
#define PWM_GEN(n)        (PWM_BASE + 0x040u + 0x40u * (uint32_t)(n))
#define PWMn_CTL(g)   REG32(PWM_GEN(g) + 0x00u)
#define PWMn_INTEN(g) REG32(PWM_GEN(g) + 0x04u)
#define PWMn_RIS(g)   REG32(PWM_GEN(g) + 0x08u)
#define PWMn_ISC(g)   REG32(PWM_GEN(g) + 0x0Cu)
#define PWMn_LOAD(g)  REG32(PWM_GEN(g) + 0x10u)
#define PWMn_COUNT(g) REG32(PWM_GEN(g) + 0x14u)
#define PWMn_CMPA(g)  REG32(PWM_GEN(g) + 0x18u)
#define PWMn_CMPB(g)  REG32(PWM_GEN(g) + 0x1Cu)
#define PWMn_GENA(g)  REG32(PWM_GEN(g) + 0x20u)
#define PWMn_GENB(g)  REG32(PWM_GEN(g) + 0x24u)

#define PWMn_CTL_ENABLE (1u << 0)
/* ACTCMPAD = drive LOW on compare-A-down, ACTLOAD = drive HIGH on load:
 * a plain left-aligned duty cycle. Values are the 2-bit action codes,
 * 2 = drive low, 3 = drive high. */
#define PWM_GENA_LOAD_HIGH  (3u << 6)
#define PWM_GENA_CMPAD_LOW  (2u << 10)
#define PWM_GENB_LOAD_HIGH  (3u << 6)
#define PWM_GENB_CMPBD_LOW  (2u << 14)

/* ── General-purpose timers (GPTM) ────────────────────────────────────────
 * Needed because the PWM counters are only 16 BITS. At 50 MHz that is 1.31 ms
 * of range, and IR gaps routinely run tens of milliseconds — so the carrier can
 * live on PWM but the BURST timing cannot. A GPTM pair concatenated into 32-bit
 * mode gives 85 s of range at the same clock, which is the right tool.
 *
 * These four base addresses are confirmed: all of TIMER0..TIMER3 appear as
 * literals in the stock image's per-processor config blocks.
 */
#define TIMER0_BASE 0x40030000u
#define TIMER1_BASE 0x40031000u
#define TIMER2_BASE 0x40032000u
#define TIMER3_BASE 0x40033000u

#define GPTM_CFG(base)   REG32((base) + 0x000u)
#define GPTM_TAMR(base)  REG32((base) + 0x004u)
#define GPTM_CTL(base)   REG32((base) + 0x00Cu)
#define GPTM_IMR(base)   REG32((base) + 0x018u)
#define GPTM_RIS(base)   REG32((base) + 0x01Cu)
#define GPTM_ICR(base)   REG32((base) + 0x024u)
#define GPTM_TAILR(base) REG32((base) + 0x028u)
#define GPTM_TAR(base)   REG32((base) + 0x048u)

#define GPTM_CFG_32BIT   0x00000000u   /* one 32-bit timer, not two 16-bit */
#define GPTM_TAMR_ONESHOT 0x1u
#define GPTM_TAMR_PERIODIC 0x2u
#define GPTM_CTL_TAEN    (1u << 0)
#define GPTM_RIS_TATORIS (1u << 0)     /* Timer A time-out raw interrupt */

/* RCGC1 timer bits: TIMER0..TIMER3 are 16..19. */
#define RCGC1_TIMER(n) (1u << (16u + (n)))

/* ── NVIC (Cortex-M3 core, identical to M4) ───────────────────────────────── */
#define NVIC_ISER(n) REG32(0xE000E100u + 4u * (uint32_t)(n))
#define NVIC_ICER(n) REG32(0xE000E180u + 4u * (uint32_t)(n))
#define SCB_VTOR     REG32(0xE000ED08u)

static inline void nvic_enable(uint32_t irq) { NVIC_ISER(irq >> 5) = 1u << (irq & 31u); }

/* ── Clocking ─────────────────────────────────────────────────────────────── */
/* 50 MHz: the PLL runs at 400 MHz / 2 = 200 MHz and SYSDIV divides by 4.
 * The same figure the EA firmware uses, so ir_engine's tick arithmetic and the
 * carrier maths carry over unchanged. */
#define OHC_SYSCLK_HZ 50000000u

/* Host link baud. THIS IS THE ONE THAT DIFFERS FROM THE EA — measured, not
 * assumed: a live HC-800 answered FIRMWARE_VERSION_GET and PRODUCT_NAME on
 * /dev/ttyS3 at 115200 with a well-formed reply. The EA family runs 460800. */
#define OHC_HOST_BAUD 115200u

/* ── HAL, implemented in src/hal_lm3s.c ───────────────────────────────────── */
void lm3s_clock_init(void);
void lm3s_clock_gate(volatile uint32_t *rcgc, uint32_t bit);
void lm3s_uart_init(uint32_t base, uint32_t baud);
void lm3s_uart_putc(uint32_t base, uint8_t c);
int  lm3s_uart_getc(uint32_t base);          /* -1 when the RX FIFO is empty */
void lm3s_delay_us(uint32_t us);

/* IR carrier, on the PWM module. Same signatures the EA HAL exposes so
 * src/main.c and ir_engine stay identical across the two parts. */
void ir_carrier_configure(uint8_t channel, uint32_t carrier_ticks);
void ir_carrier_set(uint8_t channel, bool on);
void ir_burst_timer_start(uint32_t ticks);
bool ir_burst_timer_expired(void);

#endif /* OHC_LM3S_H */
