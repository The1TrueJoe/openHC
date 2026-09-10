/* Board profile for the LM3S1162 IO processor (HC-800, HC-250).
 *
 * Control4's `.flash.config` maps **hc800 and hc250 to the same pair** of
 * images, exactly as it maps ea1/ea3/ea5 to one TM4C pair. The stock LM3S image
 * is named "IoProcMultiConfig1162" and carries FOUR config blocks at flash
 * 0x10B8, stride 0x3D4 — but note what varies across them:
 *
 *     Processor: LM3S1162 / LM3S615 / LM3S811 / LM3S815 / Undefined
 *
 * Those are the strings in the image (flash 0x7931..0x7985). "MultiConfig"
 * here means multi-PROCESSOR, not multi-board the way the TM4C image's ADC
 * strap does — the vendor built one binary that runs on four different
 * Stellaris parts. We claim only the LM3S1162, so we need none of that
 * machinery and simply build for the part in front of us.
 *
 * ── Counts ────────────────────────────────────────────────────────────────
 *
 *              IR out   relays   contacts   user UARTs
 *   hc800         6        4         4          0
 *
 * IR = 6: the owner counts six rear jacks, and the stock image's vector table
 * backs that up — it claims PWM generators 0, 1 and 2 plus PWM Fault, and three
 * generators drive two outputs each. That is what settles the count against the
 * older reading of "four, from TIMER0-3": four timers were a LOWER BOUND
 * inferred from a base-address table, whereas the PWM interrupts are the
 * carrier hardware actually being used. Six outputs, six jacks, no internal
 * blaster to account for (unlike the EA, where output count is jack count + 1).
 *
 * USER UARTS = 0, and this is the other thing the HC-800 does differently.
 * The stock image configures three UART base addresses, which previously read
 * as "the two rear RS-232 ports are MCU-routed". They are not, on this board:
 * on a live HC-800 `ioserver` holds /dev/ttyS1 (0x2f8) and /dev/ttyS2 (0x3e8)
 * — real host 16550As on the LPC bus, both showing RTS|DTR — at the same time
 * as it holds /dev/ttyS3 for the MCU. The rear jacks are wired to the HOST.
 * The image's third UART belongs to the HC-250 half of the MultiConfig set.
 *
 * So this MCU's whole job is IR out, IR in, 4 relays and 4 contacts.
 *
 * CONFIRMING TEST (not yet done): short pins 2 and 3 on a rear DB9 and echo
 * through /dev/ttyS1. If it loops back, the host owns the jack outright.
 */
#ifndef OHC_BOARD_PROFILE_H
#define OHC_BOARD_PROFILE_H

#define OHC_BOARD_HC800 1
#define OHC_BOARD_HC250 2

#ifndef OHC_BOARD
#define OHC_BOARD OHC_BOARD_HC800
#endif

#if OHC_BOARD == OHC_BOARD_HC800

#define OHC_BOARD_NAME       "hc800"
#define OHC_IR_OUT_COUNT     6
#define OHC_IR_JACK_COUNT    6
#define OHC_USER_UART_COUNT  0      /* rear RS-232 is host ttyS1/ttyS2 */
#define OHC_RELAY_COUNT      4
#define OHC_CONTACT_COUNT    4

#elif OHC_BOARD == OHC_BOARD_HC250

/* The HC-250 shares this MCU and this image but is an ARMv7 host with a
 * different panel. Counts here are NOT verified — no HC-250 has been opened —
 * so the board is refused rather than built wrong. Fill these in from a real
 * unit's panel and delete the #error.
 */
#error "hc250 profile is unmeasured — see the note in include/board_profile.h"

#else
#error "OHC_BOARD must be OHC_BOARD_HC800 (build with BOARD=hc800)"
#endif

/* Dense mask of populated outputs — channels run contiguously from 0. */
#define OHC_IR_OUT_MASK ((1u << OHC_IR_OUT_COUNT) - 1u)

#if OHC_IR_OUT_COUNT > 6
#error "OHC_IR_OUT_COUNT exceeds the 6 PWM outputs (3 generators x 2)"
#endif

#endif /* OHC_BOARD_PROFILE_H */
