/* SPDX-License-Identifier: MIT */
/*
 * LM3S1162: what each pin can do, from StellarisWare's pin_map.h (PART_LM3S1162).
 * The carrier comes from the PWM module (outputs 0..5, generator n drives 2n
 * and 2n+1); a pin has one alternate function, so there is no mux value. The
 * receiver takes any pin: a GPIO edge interrupt timestamped from a 32-bit timer.
 *
 * Ports E..H are not narrowed to the package: an unbonded pin there would be
 * accepted and do nothing.
 */
#include "ohc-iomcu-proto.h"
#include "part.h"

#define A 0
#define C 2
#define D 3
#define F 5
#define G 6
#define H 7

#define PWM(p, n, u)   { p, n, PF_CARRIER, u, 0 }

static const pin_func funcs[] = {
    PWM(D, 0, 0), PWM(D, 1, 1), PWM(H, 0, 2), PWM(H, 1, 3), PWM(F, 2, 4), PWM(F, 3, 5),
    { D, 2, PF_UART_RX, 1, 0 }, { D, 3, PF_UART_TX, 1, 0 },
    { G, 0, PF_UART_RX, 2, 0 }, { G, 1, PF_UART_TX, 2, 0 },
};

const part_desc part_lm3s1162 = {
    .name = "lm3s1162",
    .id = OHC_PART_LM3S1162,
    .bonded   = { 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff },
    /* PA0/PA1 = UART0, the host link; PC0..PC3 = JTAG/SWD; PB7 = TRST */
    .reserved = { 0x03, 0x80, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00 },
    .funcs = funcs,
    .n_funcs = sizeof(funcs) / sizeof(funcs[0]),
    .user_uarts = 2,
    .capture_any_gpio = 1,
    .host_baud = 115200,
};
