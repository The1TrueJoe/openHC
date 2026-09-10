/* IR / relay / contact pin map for the LM3S1162 IO processor.
 *
 * ── READ THIS BEFORE FLASHING ─────────────────────────────────────────────
 *
 * The GPIO PIN ASSIGNMENTS BELOW ARE NOT YET KNOWN. Everything else in this
 * firmware is derived from something measured — the protocol from a live
 * exchange with the MCU, the image layout from the vendor binary, the channel
 * count from the vendor's PWM interrupt vectors — but nobody has yet decoded
 * which package pin carries which IR jack, relay or contact.
 *
 * That is deliberate and it is marked, rather than filled in with plausible
 * guesses that would look identical to fact three months from now. Driving a
 * wrong pin is not harmless here: the relays may be wired to real loads.
 *
 * WHAT IS KNOWN, and it is most of the way there:
 *
 *   * The carrier comes from the PWM module, not a timer CCP. The stock image's
 *     vector table claims PWM Fault and PWM generators 0, 1 and 2 (IRQs 9-12)
 *     and claims no timer interrupt at all. Three generators x two outputs =
 *     the six IR channels.
 *   * Generator n drives outputs PWM(2n) and PWM(2n+1), so the channel -> 
 *     generator mapping below is fixed by the silicon and needs no decoding.
 *   * The vendor's per-processor config blocks (flash 0x10B8, stride 0x3D4)
 *     carry the pin descriptors, and THE RECORD FORMAT IS NOW DECODED:
 *
 *         GPIO pin record   8 bytes   { u32 gpio_base; u32 pin }
 *                                     pin bits 0..7 = the mask,
 *                                     BIT 8 = POPULATED (same convention the
 *                                     TM4C table uses)
 *         timer record     12 bytes   { u32 timer_base; u32 mask; u32 mask2 }
 *                                     four per block, always in the order
 *                                     TIMER2, TIMER0, TIMER3, TIMER1
 *
 *     Populated pins per block, in file order:
 *
 *       block 0  32  PD4 PC7 PC6 PF4 PC5 PC4 PF5 PB4 PB5 PA7 PB6 PF2 PA6 ...
 *       block 1  28  PD4 PC7 PC6 PF4 PC5 PC4 PF5 PB4 PB5 PA7 PB6 PF2 PA6 ...
 *       block 2  38  PD4 PC7 PC6 PF4 PC5 PC4 PF5 PB4 PB5 PA7 PB6 PF2 PA6 ...
 *       block 3   6  PA0 PA1 PD2 PD3 PG0 PG1
 *
 *     TWO THINGS ARE NOW SOLID:
 *
 *       - The LAST SIX pins of every block are the three UARTs, and they are
 *         the textbook Stellaris pinouts: UART0 = PA0/PA1, UART1 = PD2/PD3,
 *         UART2 = PG0/PG1. Block 3 — the "Undefined" processor — contains
 *         ONLY those six, which is exactly what an unknown part should get:
 *         a host link and nothing else. That agreement is what confirms the
 *         record format rather than merely fitting it.
 *       - Blocks 0, 1 and 2 share the same FIRST THIRTEEN pins, so a profile
 *         is a prefix length here too, not a remapping.
 *
 *     AND ONE THING IS NOT, WHICH IS WHY THE TABLE BELOW IS STILL EMPTY:
 *
 *     The obvious reading of that prefix is PD4 = IR receiver (it is the first
 *     descriptor and it sits among the timer records, mirroring the TM4C
 *     layout where the receiver comes first and owns a capture timer), then
 *     PC7 PC6 PF4 PC5 PC4 PF5 = the six IR outputs. Six is the right number
 *     and the position is right.
 *
 *     But PC4..PC7 are classically the CCP (timer capture/compare) pins on
 *     Stellaris, not PWM pins — while the stock vector table says the carrier
 *     runs on PWM generators 0/1/2 and takes NO timer interrupt at all. Those
 *     two facts do not sit together, and until they do, assigning those pins
 *     is a guess dressed as a decode. Resolving it needs the LM3S1162 pin
 *     table (datasheet Table "Signals by Function", or StellarisWare's
 *     pin_map.h under PART_LM3S1162) — neither was reachable from here.
 *
 *     Which block is the LM3S1162 is also unsettled: the image's strings run
 *     LM3S615, LM3S815, LM3S811, LM3S1162, but block 3 is the minimal one, so
 *     the block order is NOT the string order and the mapping is unproven.
 *
 * Until then ir_carrier_configure() refuses to touch a pin (see hal_lm3s.c):
 * the carrier is set up on the PWM generator, which is safe, and the GPIO
 * enable that would actually drive a jack is gated on OHC_IR_PINS_KNOWN.
 * Define that only when the table below is real.
 */
#ifndef OHC_IR_PINS_H
#define OHC_IR_PINS_H

#include <stdint.h>

#include "board_profile.h"
#include "lm3s.h"

/* Flip to 1 ONLY when the gpio_base/pin_mask columns below are measured. While
 * it is 0 the firmware runs, answers the host and generates carriers, but never
 * enables an output pin — so it is safe to flash on a unit wired to real gear. */
#define OHC_IR_PINS_KNOWN 0

typedef struct {
    uint8_t  pwm_gen;      /* PWM generator 0..2 */
    uint8_t  pwm_out;      /* 0 = output A (PWM 2n), 1 = output B (PWM 2n+1) */
    uint32_t gpio_base;    /* UNKNOWN — see header */
    uint8_t  pin_mask;     /* UNKNOWN — see header */
} ir_channel;

#define IR_CHANNEL_COUNT 6

/* Generator/output columns are silicon facts. GPIO columns are placeholders and
 * are inert while OHC_IR_PINS_KNOWN is 0. */
static const ir_channel IR_CHANNELS[IR_CHANNEL_COUNT] = {
    { 0, 0, 0, 0 },   /* 0: PWM0, gen 0 output A */
    { 0, 1, 0, 0 },   /* 1: PWM1, gen 0 output B */
    { 1, 0, 0, 0 },   /* 2: PWM2, gen 1 output A */
    { 1, 1, 0, 0 },   /* 3: PWM3, gen 1 output B */
    { 2, 0, 0, 0 },   /* 4: PWM4, gen 2 output A */
    { 2, 1, 0, 0 },   /* 5: PWM5, gen 2 output B */
};

/* PWMENABLE bit for a channel: output index 2*gen + out. */
#define IR_PWM_ENABLE_BIT(ch) \
    (1u << (2u * IR_CHANNELS[ch].pwm_gen + IR_CHANNELS[ch].pwm_out))

/* ── burst timing ──────────────────────────────────────────────────────────
 * The transmit path needs a time base for each burst, and it CANNOT be a PWM
 * generator: those counters are 16-bit, which at 50 MHz is 1.31 ms of range,
 * while IR gaps routinely run tens of milliseconds. An earlier draft used PWM
 * generator 3 for this and would have silently truncated every long gap.
 *
 * So it is a GPTM in 32-bit one-shot mode — 85 s of range at the same clock.
 * TIMER0, which no IR channel can claim because the carriers are all on PWM.
 * Used purely as a polled down-counter: load the burst length, watch RIS for
 * the time-out. No interrupt, no output pin.
 */
#define IR_BURST_TIMER_BASE TIMER0_BASE
#define IR_BURST_TIMER_RCGC 0u          /* RCGC1_TIMER(0) */

/* ── relays and contacts ───────────────────────────────────────────────────
 * Four of each, confirmed from the panel by the owner and consistent with the
 * host-side board.env (OHC_RELAYS=4, OHC_CONTACTS=4).
 *
 * Pins UNKNOWN, same as the IR table and for the same reason — and the stakes
 * are higher: a relay may be switching a real load. Nothing in this firmware
 * drives a relay while OHC_RELAY_PINS_KNOWN is 0; RELAY_TOGGLE is accepted and
 * answered so the host sees a live device, and the state it reports is the
 * state it is tracking, not a pin it has driven.
 *
 * A CLOSED contact reads 1 in the protocol's u32 bitmask — established on a
 * live EA3 by shorting the input, and the wire format is shared.
 */
#define OHC_RELAY_PINS_KNOWN 0

typedef struct {
    uint32_t gpio_base;   /* UNKNOWN */
    uint8_t  pin_mask;    /* UNKNOWN */
} io_pin;

static const io_pin RELAY_PINS[OHC_RELAY_COUNT] = {
    { 0, 0 }, { 0, 0 }, { 0, 0 }, { 0, 0 },
};

static const io_pin CONTACT_PINS[OHC_CONTACT_COUNT] = {
    { 0, 0 }, { 0, 0 }, { 0, 0 }, { 0, 0 },
};

/* ── host link ─────────────────────────────────────────────────────────────
 * UART0 at 115200 8N1. The stock image's vector table claims UART0 and UART1;
 * UART0 is the host side (it is what answered our probe on /dev/ttyS3), and
 * UART1 belongs to the HC-250 half of the MultiConfig image — this board's rear
 * RS-232 ports are host 16550As, not MCU-routed. See board_profile.h.
 *
 * PIN ASSIGNMENT UNKNOWN, like everything else here, but it does not block
 * bring-up: the bootloader already speaks to the host over this UART to accept
 * a firmware image, so the pins are configured by the bootloader before our
 * reset vector runs. hal_lm3s.c therefore re-initialises the UART registers but
 * does NOT touch the GPIO mux for them — inheriting a working configuration is
 * both safer and one less thing to get wrong.
 */
#define UART_HOST_BASE UART0_BASE

#endif /* OHC_IR_PINS_H */
