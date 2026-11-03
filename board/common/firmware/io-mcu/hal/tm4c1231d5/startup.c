/* SPDX-License-Identifier: MIT */
/*
 * Vector table and reset. Linked at 0x1000: the TI bootloader owns the low
 * 4 KB, and nothing here may land there. 16 system entries + IRQ 0..106.
 */
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;
int main(void);

void systick_isr(void);
void uart0_isr(void);
void uart1_isr(void);
void uart2_isr(void);
void uart3_isr(void);
void uart4_isr(void);
void uart5_isr(void);
void uart6_isr(void);
void uart7_isr(void);
void burst_isr(void);
void capture_isr(void);

static void spin(void)
{
    for (;;)
        ;
}

/* A fault or a stray interrupt: reset. The bootloader does not start the app
 * by itself, so the host finds the part there and can write it again. */
static void fault(void)
{
    *(volatile uint32_t *)0xE000ED0Cu = 0x05FA0004u;   /* AIRCR: SYSRESETREQ */
    spin();
}

void reset_isr(void)
{
    uint32_t *s = &_sidata, *d = &_sdata;

    while (d < &_edata)
        *d++ = *s++;
    for (d = &_sbss; d < &_ebss; d++)
        *d = 0;
    main();
    spin();
}

#define IRQ(n) [16 + (n)]

__attribute__((section(".isr_vector"), used))
void (*const vectors[16 + 107])(void) = {
    [0] = (void (*)(void))&_estack,
    [1] = reset_isr,
    [2 ... 15] = fault,
    [15] = systick_isr,
    [16 ... 16 + 106] = fault,
    IRQ(5) = uart0_isr,
    IRQ(6) = uart1_isr,
    IRQ(33) = uart2_isr,
    IRQ(59) = uart3_isr,
    IRQ(60) = uart4_isr,
    IRQ(61) = uart5_isr,
    IRQ(62) = uart6_isr,
    IRQ(63) = uart7_isr,
    IRQ(70) = burst_isr,
    IRQ(94) = capture_isr, IRQ(95) = capture_isr,   /* WTIMER0..5 A/B */
    IRQ(96) = capture_isr, IRQ(97) = capture_isr,
    IRQ(98) = capture_isr, IRQ(99) = capture_isr,
    IRQ(100) = capture_isr, IRQ(101) = capture_isr,
    IRQ(102) = capture_isr, IRQ(103) = capture_isr,
    IRQ(104) = capture_isr, IRQ(105) = capture_isr,
};
