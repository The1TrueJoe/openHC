/* IR and user-serial pin map for the TM4C1231D5PM IO processor.
 *
 * EXTRACTED FROM THE STOCK FIRMWARE, not guessed. The vendor image carries six
 * per-board config blocks at file offset 0x1fec, stride 0x4a4. Within a block:
 *
 *     +0x000  0x18 bytes  the IR RECEIVER descriptor (see IRRX_* below)
 *     +0x018  0x1c bytes  IR output, channel 0
 *     +0x034  0x1c each   IR outputs, channels 1..8
 *     +0x3b4  0x28 each   UART records (host link first, then user ports)
 *
 * An output descriptor is:
 *     +0x00  u32  GPIO port base (APB)
 *     +0x04  u32  pin mask in bits 0..7;  BIT 8 = POPULATED ON THIS BOARD
 *     +0x08  u32  timer base
 *     +0x0c  u32  exception number for timer A  (IRQ + 16)
 *     +0x10  u32  exception number for timer B, or 0xFF
 *
 * Every entry lands on the documented CCP0 pin for its timer, which is what
 * confirms the decoding. It also settles the package question: PB6 = T0CCP0
 * exists on the **PM** part and not on PZ.
 *
 * ── THE BOARD-DEPENDENCE IS RESOLVED ──────────────────────────────────────
 *
 * An earlier reading of this table said the channel order "must be measured"
 * because channels 4..7 appeared in two different orders across the six blocks.
 * That is settled now, and the answer is simpler than it looked.
 *
 * Decoding bit 8 of the pin-mask word as POPULATED turns the six blocks into
 * board profiles, and the counts then match real hardware exactly:
 *
 *     block 2 -> EA1   5 outputs, 2 user UARTs   (4 jacks + 1 blaster)
 *     block 3 -> EA3   7 outputs, 3 user UARTs   (6 jacks + 1 blaster)
 *     blocks 0,1,4,5   9 outputs, 2 user UARTs   (EA5 / TR1 / amp1)
 *
 * Both boards we target are in the {2,3,4} ordering group, so THEY SHARE ONE
 * TABLE — the one below. (The other group, blocks {0,1,5}, swaps the PC4/PC6
 * pair with the PD0/PD2 pair in the tail slots. No EA1 or EA3 uses it, so it is
 * not represented here; add it if an EA5 profile is ever needed.)
 *
 * Which channels a given board actually populates is OHC_IR_OUT_COUNT in
 * board_profile.h, not here. Both boards populate a contiguous run from
 * channel 0, so the bound is a simple prefix length.
 *
 * NOTE the interlock at channel 7: PC4 is both WT0CCP0 and UART4's RX pin.
 * EA3 uses UART4 as its third user serial port, so channel 7 CANNOT be an IR
 * output there — and indeed block 3 marks it unpopulated. OHC_IR_OUT_COUNT = 7
 * enforces this for free, but do not raise it without re-reading this note.
 *
 * WHICH REAR JACK IS WHICH is still unconfirmed. Nothing in the vendor binary
 * ties a descriptor index to a labelled jack; that mapping has to come off the
 * PCB or from driving one channel at a time.
 */
#ifndef OHC_IR_PINS_H
#define OHC_IR_PINS_H

#include <stdint.h>

#include "board_profile.h"
#include "tm4c.h"   /* GPIO*_BASE, TIMER*_BASE, UART*_BASE */

typedef struct {
    uint32_t gpio_base;
    uint8_t  pin_mask;
    uint32_t timer_base;
    uint8_t  irq_a;        /* NVIC IRQ number (exception number - 16) */
} ir_channel;

/* Physical descriptors the part provides. How many are POPULATED is
 * OHC_IR_OUT_COUNT; this is the full table both target boards index into. */
#define IR_CHANNEL_COUNT 9

/* Indices are the bit positions used by IROUT_SEND's output_mask.
 * Order is vendor blocks 2 and 3 — i.e. EA1 and EA3. Verified against the
 * stock image; see the header comment. */
static const ir_channel IR_CHANNELS[IR_CHANNEL_COUNT] = {
    { GPIOD_BASE, 0x10, WTIMER4_BASE, 102 },  /* 0: PD4  WT4CCP0  EA1 + EA3 */
    { GPIOB_BASE, 0x40, TIMER0_BASE,   19 },  /* 1: PB6  T0CCP0   EA1 + EA3 */
    { GPIOF_BASE, 0x04, TIMER1_BASE,   21 },  /* 2: PF2  T1CCP0   EA1 + EA3 */
    { GPIOB_BASE, 0x01, TIMER2_BASE,   23 },  /* 3: PB0  T2CCP0   EA1 + EA3 */
    { GPIOB_BASE, 0x04, TIMER3_BASE,   35 },  /* 4: PB2  T3CCP0   EA1 + EA3 */
    { GPIOD_BASE, 0x01, WTIMER2_BASE,  98 },  /* 5: PD0  WT2CCP0  EA3 only  */
    { GPIOD_BASE, 0x04, WTIMER3_BASE, 100 },  /* 6: PD2  WT3CCP0  EA3 only  */
    { GPIOC_BASE, 0x10, WTIMER0_BASE,  94 },  /* 7: PC4  WT0CCP0  neither
                                               *    (PC4 is UART4 RX on EA3) */
    { GPIOC_BASE, 0x40, WTIMER1_BASE,  96 },  /* 8: PC6  WT1CCP0  neither    */
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

/* ── burst timing timer ────────────────────────────────────────────────────
 * The transmit path needs one timer of its own to time each burst, and it must
 * not be a timer any IR channel claims. Across ALL SIX vendor blocks the
 * timers used are T0..T3, WT0..WT4 for outputs and WT5 for the receiver —
 * which leaves TIMER4 and TIMER5 free on every board. TIMER4 it is.
 *
 * This previously used TIMER1, which is channel 2's carrier (PF2/T1CCP0) on
 * both EA1 and EA3 — a transmission on channel 2 would have fought the burst
 * timer. Knowing the real channel set is what made the fix decidable.
 */
#define IR_BURST_TIMER_BASE  TIMER4_BASE
#define IR_BURST_TIMER_RCGC  4u          /* SYSCTL_RCGCTIMER bit */

/* ── user serial ports ─────────────────────────────────────────────────────
 * UART0 (PA0/PA1) is the host link at 460800. The user-facing RS-232 ports are
 * MCU-routed — there is no host /dev/ttyS* for them. The vendor brings them up
 * at 115200 8N1 on every board.
 *
 * How many exist is OHC_USER_UART_COUNT: 2 on EA1, 3 on EA3. UART4 is the
 * EA3-only third port, and it is the reason IR channel 7 (PC4) is unpopulated
 * there — same pin.
 */
#define UART_USER0_BASE  UART5_BASE     /* 0x40011000 */
#define UART_USER0_GPIO  GPIOE_BASE     /* PE4 = RX, PE5 = TX */
#define UART_USER0_RX    0x10u
#define UART_USER0_TX    0x20u
#define UART_USER0_IRQ   61u

#define UART_USER1_BASE  UART7_BASE     /* 0x40013000 */
#define UART_USER1_GPIO  GPIOE_BASE     /* PE0 = RX, PE1 = TX */
#define UART_USER1_RX    0x01u
#define UART_USER1_TX    0x02u
#define UART_USER1_IRQ   63u

#if OHC_USER_UART_COUNT >= 3
#define UART_USER2_BASE  UART4_BASE     /* 0x40010000 — EA3 only */
#define UART_USER2_GPIO  GPIOC_BASE     /* PC4 = RX, PC5 = TX */
#define UART_USER2_RX    0x10u
#define UART_USER2_TX    0x20u
#define UART_USER2_IRQ   60u
#endif

/* UART RX/TX pins all select PCTL value 1 on this part. */
#define UART_PCTL        1u

#endif /* OHC_IR_PINS_H */
