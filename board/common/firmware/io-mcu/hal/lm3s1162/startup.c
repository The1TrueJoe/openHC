/* SPDX-License-Identifier: MIT */
/*
 * Vector table and reset. Linked at 0x1000: the bootloader owns the low 4 KB,
 * and nothing here may land there. 16 system entries + IRQ 0..47.
 */
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;
int main(void);

void systick_isr(void);
void uart0_isr(void);
void uart1_isr(void);
void uart2_isr(void);
void burst_isr(void);
void gpio_isr(void);

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
void (*const vectors[16 + 48])(void) = {
    [0] = (void (*)(void))&_estack,
    [1] = reset_isr,
    [2 ... 15] = fault,
    [15] = systick_isr,
    [16 ... 16 + 47] = fault,
    IRQ(0) = gpio_isr, IRQ(1) = gpio_isr, IRQ(2) = gpio_isr,    /* GPIO A..E */
    IRQ(3) = gpio_isr, IRQ(4) = gpio_isr,
    IRQ(5) = uart0_isr,
    IRQ(6) = uart1_isr,
    IRQ(19) = burst_isr,
    IRQ(30) = gpio_isr, IRQ(31) = gpio_isr, IRQ(32) = gpio_isr, /* GPIO F..H */
    IRQ(33) = uart2_isr,
};
