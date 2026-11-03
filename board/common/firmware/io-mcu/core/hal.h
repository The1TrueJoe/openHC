/* SPDX-License-Identifier: MIT */
/*
 * What the application needs from a part. Each hal/<part>/ implements it; the
 * host tests implement it with a fake. Entries passed in have been validated
 * by cfg_parse against the same part.
 */
#ifndef OHC_HAL_H
#define OHC_HAL_H

#include <stdbool.h>
#include <stdint.h>

#include "config.h"
#include "part.h"

#ifndef HAL_USER_UARTS
#define HAL_USER_UARTS 7    /* static buffers; the part's own count is in part_desc */
#endif

extern const part_desc *hal_part;

void hal_init(void);
uint32_t hal_clock_hz(void);        /* timer and carrier tick rate */
uint32_t hal_millis(void);
uint32_t hal_irq_off(void);         /* returns the state hal_irq_restore takes */
void hal_irq_restore(uint32_t state);
void hal_reset(void);               /* into the bootloader; does not return on the part */

/* Host link: RX is interrupt driven; putc blocks on a full FIFO. */
int hal_host_getc(void);            /* -1 when nothing is waiting */
void hal_host_putc(uint8_t c);
void hal_host_flush(void);

/* Every configured pin back to an input, every peripheral it used off. */
void hal_release(void);

void hal_gpio_out(uint8_t port, uint8_t pin, bool level);
void hal_gpio_set(uint8_t port, uint8_t pin, bool level);
void hal_gpio_in(uint8_t port, uint8_t pin, uint8_t flags);  /* OHC_CFG_F_PULL_* */
bool hal_gpio_get(uint8_t port, uint8_t pin);

/* IR out: the pin idles low; the carrier runs inside the peripheral and is gated onto it. */
void hal_carrier_setup(const cfg_entry *e);
void hal_carrier_start(const cfg_entry *e, uint32_t period_ticks);
void hal_carrier_gate(const cfg_entry *e, bool on);
void hal_carrier_stop(const cfg_entry *e);
/* One-shot burst timer; its interrupt calls app_burst_isr(). */
void hal_burst_start(uint32_t ticks);
void hal_burst_stop(void);

/* IR in: edges call app_capture_edge() with a timestamp on hal_capture_now()'s clock. */
void hal_capture_setup(const cfg_entry *e);
void hal_capture_enable(bool on);
uint32_t hal_capture_now(void);

/* User UART `port`: pins stay inputs until enabled; RX bytes call app_uart_rx(). */
void hal_uart_setup(uint8_t port, const cfg_entry *rx, const cfg_entry *tx);
void hal_uart_enable(uint8_t port, bool on, uint32_t baud, uint8_t bits,
                     uint8_t parity, uint8_t stop);
bool hal_uart_putc(uint8_t port, uint8_t c);    /* false: FIFO full */

/* Called from interrupt handlers. */
void app_burst_isr(void);
void app_capture_edge(uint32_t t, bool level);
void app_uart_rx(uint8_t port, uint8_t c);

#endif
