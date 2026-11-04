/* SPDX-License-Identifier: MIT */
/* The UART both parts carry (ARM PL011 layout). */
#ifndef OHC_PL011_H
#define OHC_PL011_H

#include "cortexm.h"

#define UART_DR(b)    REG((b) + 0x000u)
#define UART_ECR(b)   REG((b) + 0x004u)
#define UART_FR(b)    REG((b) + 0x018u)
#define UART_IBRD(b)  REG((b) + 0x024u)
#define UART_FBRD(b)  REG((b) + 0x028u)
#define UART_LCRH(b)  REG((b) + 0x02Cu)
#define UART_CTL(b)   REG((b) + 0x030u)
#define UART_IFLS(b)  REG((b) + 0x034u)
#define UART_IM(b)    REG((b) + 0x038u)
#define UART_ICR(b)   REG((b) + 0x044u)

#define FR_BUSY   (1u << 3)
#define FR_RXFE   (1u << 4)
#define FR_TXFF   (1u << 5)
#define LCRH_PEN  (1u << 1)
#define LCRH_EPS  (1u << 2)
#define LCRH_STP2 (1u << 3)
#define LCRH_FEN  (1u << 4)
#define CTL_EN    ((1u << 0) | (1u << 8) | (1u << 9))   /* UARTEN, TXE, RXE */
#define IM_RX     ((1u << 4) | (1u << 6))               /* RX, RX timeout */
#define IFLS_RX_HALF (2u << 3)

static inline void pl011_wait_idle(uint32_t b)
{
    while (UART_FR(b) & FR_BUSY)
        ;
}

/* Line format; the divisor is written too, as LCRH latches it. */
static inline void pl011_setup(uint32_t b, uint32_t clk, uint32_t baud, uint8_t bits,
                               uint8_t parity, uint8_t stop)
{
    uint32_t brd = (clk * 4u + baud / 2u) / baud;   /* 64ths of 16x */
    uint32_t lcrh = ((uint32_t)(bits - 5u) << 5) | LCRH_FEN;

    if (parity)
        lcrh |= LCRH_PEN | (parity == 2 ? LCRH_EPS : 0u);
    if (stop == 2)
        lcrh |= LCRH_STP2;
    UART_CTL(b) = 0;
    UART_IBRD(b) = brd / 64u;
    UART_FBRD(b) = brd % 64u;
    UART_LCRH(b) = lcrh;
    UART_IFLS(b) = IFLS_RX_HALF;
    UART_IM(b) = IM_RX;
    UART_CTL(b) = CTL_EN;
}

static inline bool pl011_putc(uint32_t b, uint8_t c)
{
    if (UART_FR(b) & FR_TXFF)
        return false;
    UART_DR(b) = c;
    return true;
}

static inline int pl011_getc(uint32_t b)
{
    if (UART_FR(b) & FR_RXFE)
        return -1;
    return (int)(UART_DR(b) & 0xffu);   /* error bits dropped with the byte's flags */
}

static inline void pl011_ack(uint32_t b)
{
    UART_ICR(b) = IM_RX;
    UART_ECR(b) = 0;
}

#endif
