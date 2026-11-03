/* SPDX-License-Identifier: MIT */
/*
 * TM4C1231D5PM (64-pin): what each pin can do, from the TM4C123x datasheet's
 * GPIO pins and alternate functions table. No PWM module, so the IR carrier
 * is a timer CCP in PWM mode; capture needs a 32-bit (wide) timer half.
 *
 * Timer units: T0A=0, T0B=1 ... T5B=11, WT0A=12 ... WT5B=23. TIMER4 times
 * bursts, TIMER5 is unused; both only reach the JTAG pins PC0..PC3.
 */
#include "ohc-iomcu-proto.h"
#include "part.h"

#define A 0
#define B 1
#define C 2
#define D 3
#define E 4
#define F 5

#define T(p, n, u)  { p, n, PF_CARRIER, u, 7 }               /* 16-bit timer CCP */
#define WT(p, n, u) { p, n, PF_CARRIER | PF_CAPTURE, u, 7 }  /* 32-bit timer CCP */
#define RX(p, n, u, m) { p, n, PF_UART_RX, u, m }
#define TX(p, n, u, m) { p, n, PF_UART_TX, u, m }

static const pin_func funcs[] = {
    T(B, 0, 4), T(B, 1, 5), T(B, 2, 6), T(B, 3, 7),
    T(B, 4, 2), T(B, 5, 3), T(B, 6, 0), T(B, 7, 1),
    WT(C, 4, 12), WT(C, 5, 13), WT(C, 6, 14), WT(C, 7, 15),
    WT(D, 0, 16), WT(D, 1, 17), WT(D, 2, 18), WT(D, 3, 19),
    WT(D, 4, 20), WT(D, 5, 21), WT(D, 6, 22), WT(D, 7, 23),
    T(F, 0, 0), T(F, 1, 1), T(F, 2, 2), T(F, 3, 3), T(F, 4, 4),

    RX(B, 0, 1, 1), TX(B, 1, 1, 1),
    RX(C, 4, 4, 1), TX(C, 5, 4, 1),
    RX(C, 4, 1, 2), TX(C, 5, 1, 2),
    RX(C, 6, 3, 1), TX(C, 7, 3, 1),
    RX(D, 4, 6, 1), TX(D, 5, 6, 1),
    RX(D, 6, 2, 1), TX(D, 7, 2, 1),
    RX(E, 0, 7, 1), TX(E, 1, 7, 1),
    RX(E, 4, 5, 1), TX(E, 5, 5, 1),
};

const part_desc part_tm4c1231d5 = {
    .name = "tm4c1231d5",
    .id = OHC_PART_TM4C1231D5,
    /*            A     B     C     D     E     F */
    .bonded   = { 0xff, 0xff, 0xff, 0xff, 0x3f, 0x1f, 0, 0 },
    /* PA0/PA1 = UART0, the host link; PC0..PC3 = JTAG/SWD */
    .reserved = { 0x03, 0x00, 0x0f, 0x00, 0x00, 0x00, 0, 0 },
    .funcs = funcs,
    .n_funcs = sizeof(funcs) / sizeof(funcs[0]),
    .user_uarts = 7,
    .capture_any_gpio = 0,
    .host_baud = 460800,
};
