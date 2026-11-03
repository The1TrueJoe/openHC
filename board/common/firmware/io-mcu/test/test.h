/* SPDX-License-Identifier: MIT */
/* Minimal host test harness, plus the fake HAL's knobs. */
#ifndef OHC_TEST_H
#define OHC_TEST_H

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "config.h"
#include "proto.h"

extern int t_failed, t_checks;

#define CHECK(c) do { t_checks++; if (!(c)) { t_failed++; \
    fprintf(stderr, "%s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #c); } } while (0)
#define CHECK_EQ(a, b) do { long long _a = (long long)(a), _b = (long long)(b); t_checks++; \
    if (_a != _b) { t_failed++; fprintf(stderr, "%s:%d: %s == %lld, want %s == %lld\n", \
    __FILE__, __LINE__, #a, _a, #b, _b); } } while (0)
#define CHECK_MEM(a, b, n) do { t_checks++; if (memcmp((a), (b), (n))) { t_failed++; \
    fprintf(stderr, "%s:%d: %s differs from %s\n", __FILE__, __LINE__, #a, #b); \
    t_hexdump("got ", (const uint8_t *)(a), (n)); t_hexdump("want", (const uint8_t *)(b), (n)); } } while (0)

void t_hexdump(const char *tag, const uint8_t *p, size_t n);

void test_proto(void);
void test_ir(void);
void test_config(void);
void test_app(void);

/* ---- fake HAL ---- */

enum pin_mode { PIN_UNUSED, PIN_IN, PIN_OUT, PIN_UART };

struct fake_pin {
    enum pin_mode mode;
    bool level;         /* driven (out) or applied (in) */
    uint8_t pull;
    bool gated;         /* carrier on the pin */
};

struct fake {
    struct fake_pin pin[8][8];
    uint8_t in[4096];
    size_t in_len, in_pos;
    uint8_t out[16384];
    size_t out_len;
    uint32_t clock;
    uint32_t millis;
    uint32_t now;           /* capture clock */
    bool burst_armed;
    uint32_t burst_ticks;
    uint32_t carrier_period[24];    /* per IR channel unit, 0 = stopped */
    bool capture_on;
    bool uart_on[8];
    uint32_t uart_baud[8];
    uint8_t uart_tx[8][512];
    size_t uart_tx_len[8];
    int uart_fifo_room;     /* putc accepts this many per call to uart_poll, -1 = unlimited */
    int resets;
};
extern struct fake fk;

void fake_reset(const part_desc *part);
void fake_send(const uint8_t *bytes, size_t n);
/* Frames the firmware wrote since the last call, decoded. */
int fake_frames(ohc_frame *out, uint8_t store[][OHC_MAX_PAYLOAD], int max);

#endif
