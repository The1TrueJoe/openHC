/* SPDX-License-Identifier: MIT */
/* What a part's pins can do: the data the config frame is checked against. */
#ifndef OHC_PART_H
#define OHC_PART_H

#include <stdint.h>

/* What a pin_func row offers. */
#define PF_CARRIER  0x01    /* IR carrier output (timer CCP / PWM) */
#define PF_CAPTURE  0x02    /* edge-time capture */
#define PF_UART_RX  0x04
#define PF_UART_TX  0x08

typedef struct {
    uint8_t port, pin;
    uint8_t funcs;
    uint8_t unit;   /* timer half / PWM output / UART number */
    uint8_t mux;    /* port control value; 0 where the pin has one function */
} pin_func;

typedef struct {
    const char *name;
    uint8_t id;                 /* OHC_PART_* */
    uint8_t bonded[8];          /* per port, the pins the package has */
    uint8_t reserved[8];        /* host link, debug */
    const pin_func *funcs;
    uint8_t n_funcs;
    uint8_t user_uarts;         /* UART_* ports: 0..user_uarts-1 */
    uint8_t capture_any_gpio;   /* receiver on any pin (GPIO edge interrupt) */
    uint32_t host_baud;
} part_desc;

extern const part_desc part_tm4c1231d5;
extern const part_desc part_lm3s1162;

#endif
