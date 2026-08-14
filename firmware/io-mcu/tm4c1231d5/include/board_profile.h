/* Board profile for the TM4C1231D5 IO processor.
 *
 * One firmware, several boards — the same arrangement the stock image uses.
 * Control4's `.flash.config` maps ea1, ea3 and ea5 to a single .bin, and that
 * image carries six board profiles at file offset 0x1fec (stride 0x4a4),
 * picking one at runtime. We carry the same profiles as data and pick one at
 * COMPILE time, because the runtime selector is not yet reverse-engineered
 * far enough to use (see "Selecting at runtime" below).
 *
 * Everything here was decoded from the stock image, not guessed. The decode and
 * the evidence are in docs/io-mcu-firmware.md#the-per-board-profile-table.
 *
 *   make fw BOARD=ea1
 *   make fw BOARD=ea3
 *
 * ── What actually varies ──────────────────────────────────────────────────
 *
 * Only counts. Every board uses the same pins in the same order for the
 * channels it does populate, so a profile is a *prefix length*, not a
 * remapping:
 *
 *              IR out   IR jacks   user UARTs   relays   contacts
 *   EA1          5         4           2          0         0
 *   EA3          7         6           3          1         1
 *
 * The extra IR output on each board is an internal front blaster, which is why
 * the output count is one more than the rear-jack count.
 *
 * Because both boards populate a contiguous run from channel 0, IROUT_SEND's
 * output_mask is dense: 0x1f on EA1, 0x7f on EA3.
 *
 * ── Selecting at runtime (decoded, not yet implemented) ───────────────────
 *
 * The stock firmware picks its block from an analogue board-ID strap. The
 * selector has now been disassembled out of the vendor image (app 1.0.36, at
 * runtime address 0x2878) and it is simple:
 *
 *     ADCSequenceStepConfigure(ADC0, 0, 0, 0x6a)   0x6a = CH10|IE|END -> AIN10
 *     ADCHardwareOversampleConfigure(ADC0, 8)      each sample is an 8x average
 *     repeat 5 conversions, keeping only the LAST   (a settle-and-discard loop:
 *                                                    the scratch word is
 *                                                    overwritten, never summed)
 *     id = adc / 250;  if (adc % 250 >= 126) id++;  i.e. round-to-nearest
 *
 * So the board id is simply the strap voltage in units of 250 ADC counts, and
 * the id IS the block index — 21 separate call sites multiply it by the 0x4a4
 * stride to reach a profile.
 *
 * That makes the mapping predictable, on a 12-bit ADC against a 3.3 V
 * reference:
 *
 *     board id N  <->  adc ~= N * 250 counts  <->  ~= N * 0.20 V on PB4
 *     EA1 (block 2)    adc ~= 500             ~= 0.40 V
 *     EA3 (block 3)    adc ~= 750             ~= 0.60 V
 *
 * Those two voltages are PREDICTIONS from the decoded arithmetic, not
 * measurements — nobody has put a meter on PB4 yet. Confirm them and this
 * firmware can drop the compile-time switch and ship one image for both
 * boards, exactly as the vendor does. The profile table below is already
 * indexed the same way, so that change is small.
 */
#define OHC_BOARDID_ADC_CHANNEL   10u    /* AIN10, on PB4 */
#define OHC_BOARDID_ADC_OVERSAMPLE 8u
#define OHC_BOARDID_ADC_PER_ID    250u   /* counts per board-id step */
#define OHC_BOARDID_ADC_ROUND     126u   /* remainder >= this rounds up */
#define OHC_BOARDID_SETTLE_READS  5u     /* conversions; only the last is used */
#ifndef OHC_BOARD_PROFILE_H
#define OHC_BOARD_PROFILE_H

#define OHC_BOARD_EA1 1
#define OHC_BOARD_EA3 3

#ifndef OHC_BOARD
#define OHC_BOARD OHC_BOARD_EA1
#endif

#if OHC_BOARD == OHC_BOARD_EA1

#define OHC_BOARD_NAME       "ea1"
#define OHC_BOARD_BLOCK      2      /* vendor profile block this mirrors */
#define OHC_IR_OUT_COUNT     5      /* 4 rear jacks + 1 internal blaster */
#define OHC_IR_JACK_COUNT    4
#define OHC_USER_UART_COUNT  2
#define OHC_RELAY_COUNT      0
#define OHC_CONTACT_COUNT    0

#elif OHC_BOARD == OHC_BOARD_EA3

#define OHC_BOARD_NAME       "ea3"
#define OHC_BOARD_BLOCK      3
#define OHC_IR_OUT_COUNT     7      /* 6 rear jacks + 1 internal blaster */
#define OHC_IR_JACK_COUNT    6
#define OHC_USER_UART_COUNT  3
#define OHC_RELAY_COUNT      1
#define OHC_CONTACT_COUNT    1

#else
#error "OHC_BOARD must be OHC_BOARD_EA1 or OHC_BOARD_EA3 (build with BOARD=ea1|ea3)"
#endif

/* Dense mask of populated outputs — both known boards start at channel 0. */
#define OHC_IR_OUT_MASK ((1u << OHC_IR_OUT_COUNT) - 1u)

/* Sanity: the pin table must be able to describe every populated channel. */
#if OHC_IR_OUT_COUNT > 9
#error "OHC_IR_OUT_COUNT exceeds the 9 physical IR descriptors"
#endif
#if OHC_USER_UART_COUNT > 3
#error "OHC_USER_UART_COUNT exceeds the 3 user UARTs the part exposes"
#endif

#endif /* OHC_BOARD_PROFILE_H */
