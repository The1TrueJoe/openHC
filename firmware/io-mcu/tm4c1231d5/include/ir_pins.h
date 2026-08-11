/* IR pin map for the TM4C1231D5PM IO processor.
 *
 * EXTRACTED FROM THE STOCK FIRMWARE, not guessed. The vendor image carries a
 * per-board config array at flash 0x1fec, six blocks of stride 0x4a4. Within a
 * block:
 *
 *     +0x00   0x18 bytes  the IR RECEIVER descriptor (see IRRX_* below)
 *     +0x18   0x1c bytes  one extra IR output (index 8)
 *     +0x34   0x1c each   eight IR outputs (indices 0..7)
 *
 * An output descriptor is:
 *     +0x00  u32  GPIO port base (APB)
 *     +0x04  u32  pin mask, with bit 8 set as a per-entry attribute
 *     +0x08  u32  timer base
 *     +0x0c  u32  exception number for timer A  (IRQ + 16)
 *     +0x10  u32  exception number for timer B, or 0xFF
 *
 * Every entry lands on the documented CCP0 pin for its timer, which is what
 * confirms the decoding. It also settles the package question: PB6 = T0CCP0
 * exists on the **PM** part and not on PZ.
 *
 * ── TWO CORRECTIONS TO THE EARLIER READING OF THIS TABLE ──────────────────
 *
 * 1. PD6 / WT5CCP0 is the IR **RECEIVER**, not output channel 0. It was listed
 *    here as an output and it is not one: it lives in the separate 0x18-byte
 *    descriptor at block+0x00, its init only muxes the pin and enables clocks
 *    (no PWM ISR is registered, unlike every real output), and WTIMER5A is
 *    configured for edge-time input capture. Driving it as an emitter would
 *    have transmitted onto the receiver pin. There are NINE outputs, not ten.
 *
 * 2. THE OUTPUT ORDER IS BOARD-DEPENDENT — DO NOT HARD-CODE IT. The vendor
 *    selects a block using a board id it reads at RUNTIME from ADC0 (AIN10 on
 *    PB4: five samples averaged, round(adc/250)). Across the six blocks,
 *    indices 0..3 and 8 agree, but 4..7 come in two different orders:
 *
 *        blocks 0,1,5:  4=PC4/WT0  5=PC6/WT1  6=PD0/WT2  7=PD2/WT3
 *        blocks 2,3,4:  4=PD0/WT2  5=PD2/WT3  6=PC4/WT0  7=PC6/WT1
 *
 *    so the pairs are swapped. Which block THIS board uses cannot be known
 *    from the image — it must be measured. Until it is, treat indices 4..7 as
 *    unordered and resolve them empirically (drive one mask bit at a time and
 *    watch which emitter fires). The table below is the blocks-0/1/5 order,
 *    kept only so the code compiles and the low channels are usable.
 *
 * WHICH JACK IS WHICH is likewise still unconfirmed. Nothing in the vendor
 * binary ties a descriptor index to a labelled rear jack; the mapping has to
 * come off the PCB or from driving one channel at a time. The EA-1 populates
 * five outputs (four rear jacks plus one internal front blaster) — see
 * OHC_IR_OUT_COUNT in the board profile, not here.
 */
#ifndef OHC_IR_PINS_H
#define OHC_IR_PINS_H

#include <stdint.h>

#include "tm4c.h"   /* GPIO*_BASE, TIMER*_BASE */

typedef struct {
    uint32_t gpio_base;
    uint8_t  pin_mask;
    uint32_t timer_base;
    uint8_t  irq_a;        /* NVIC IRQ number (exception number - 16) */
} ir_channel;

#define IR_CHANNEL_COUNT 9

/* Indices are the bit positions used by IROUT_SEND's output_mask.
 * Indices 4..7 are BOARD-DEPENDENT — see the header comment before trusting them. */
static const ir_channel IR_CHANNELS[IR_CHANNEL_COUNT] = {
    { GPIOD_BASE, 0x10, WTIMER4_BASE, 102 },  /* 0: PD4  WT4CCP0 */
    { GPIOB_BASE, 0x40, TIMER0_BASE,   19 },  /* 1: PB6  T0CCP0  */
    { GPIOF_BASE, 0x04, TIMER1_BASE,   21 },  /* 2: PF2  T1CCP0  */
    { GPIOB_BASE, 0x01, TIMER2_BASE,   23 },  /* 3: PB0  T2CCP0  */
    { GPIOB_BASE, 0x04, TIMER3_BASE,   35 },  /* 4: PB2  T3CCP0  */
    { GPIOC_BASE, 0x10, WTIMER0_BASE,  94 },  /* 5: PC4  WT0CCP0  (order varies) */
    { GPIOC_BASE, 0x40, WTIMER1_BASE,  96 },  /* 6: PC6  WT1CCP0  (order varies) */
    { GPIOD_BASE, 0x01, WTIMER2_BASE,  98 },  /* 7: PD0  WT2CCP0  (order varies) */
    { GPIOD_BASE, 0x04, WTIMER3_BASE, 100 },  /* 8: PD2  WT3CCP0  (order varies) */
};

/* All CCP functions on this part select PCTL value 7. */
#define IR_PCTL_TIMER 7u

/* ── IR receiver ───────────────────────────────────────────────────────────
 * PD6 / WT5CCP0, identical in all six board blocks, so this one IS a fixed
 * fact. The vendor uses 32-bit edge-TIME capture on the NEGATIVE edge with no
 * prescale, and takes no GPIO edge interrupts anywhere on the IR path.
 *
 * IRRX_TICK_HZ is OUR system clock, not the vendor's. The vendor runs the part
 * at 80 MHz; tm4c_clock_init() runs it at OHC_SYS_CLOCK_HZ. What matters is
 * that the demodulator is told the rate the capture timer actually counts at,
 * so this tracks our clock rather than copying theirs.
 */
#define IRRX_GPIO_BASE   GPIOD_BASE     /* 0x40007000, APB */
#define IRRX_PIN_MASK    0x40u          /* PD6 */
#define IRRX_PCTL        7u             /* WT5CCP0 */
#define IRRX_TIMER_BASE  WTIMER5_BASE   /* 0x4004F000, Timer A */
#define IRRX_IRQ         104u           /* WTIMER5A, exception 120 */

/* ── user serial ports ─────────────────────────────────────────────────────
 * UART0 (PA0/PA1) is the host link. UART5 and UART7 are the two user-facing
 * RS-232 ports — both on the MCU; neither is the x86's ttyS3. Identical in
 * every board block. Vendor brings the user ports up at 115200 8N1.
 *
 * (Board block 3 additionally describes a third combo jack, so "two user
 * ports" is itself board-conditional — the selector is bounded by 3, not 2.)
 */
#define UART_USER0_BASE  UART5_BASE     /* 0x40011000, IRQ 61 */
#define UART_USER0_GPIO  GPIOE_BASE     /* PE4 = RX, PE5 = TX */
#define UART_USER0_RX    0x10u
#define UART_USER0_TX    0x20u
#define UART_USER0_IRQ   61u

#define UART_USER1_BASE  UART7_BASE     /* 0x40013000, IRQ 63 */
#define UART_USER1_GPIO  GPIOE_BASE     /* PE0 = RX, PE1 = TX */
#define UART_USER1_RX    0x01u
#define UART_USER1_TX    0x02u
#define UART_USER1_IRQ   63u

/* UART RX/TX pins all select PCTL value 1 on this part. */
#define UART_PCTL        1u

#endif /* OHC_IR_PINS_H */
