/* SPDX-License-Identifier: MIT */
/* Cortex-M3/M4 core: NVIC, SysTick, reset, interrupt masking. */
#ifndef OHC_CORTEXM_H
#define OHC_CORTEXM_H

#include <stdbool.h>
#include <stdint.h>

#define REG(a) (*(volatile uint32_t *)(uintptr_t)(a))
#define REG8(a) (*(volatile uint8_t *)(uintptr_t)(a))

#define NVIC_ISER(n)    REG(0xE000E100u + 4u * (n))
#define NVIC_ICER(n)    REG(0xE000E180u + 4u * (n))
#define NVIC_IPR(irq)   REG8(0xE000E400u + (irq))
#define SCB_VTOR        REG(0xE000ED08u)
#define SCB_AIRCR       REG(0xE000ED0Cu)
#define SYSTICK_CTRL    REG(0xE000E010u)
#define SYSTICK_LOAD    REG(0xE000E014u)
#define SYSTICK_VAL     REG(0xE000E018u)

/* Priorities, top three bits: the burst timer pre-empts everything. */
#define PRIO_BURST   0x00u
#define PRIO_CAPTURE 0x20u
#define PRIO_UART    0x40u
#define PRIO_TICK    0x60u

static inline void nvic_enable(uint32_t irq, uint8_t prio)
{
    NVIC_IPR(irq) = prio;
    NVIC_ISER(irq >> 5) = 1u << (irq & 31u);
}

static inline void nvic_disable(uint32_t irq)
{
    NVIC_ICER(irq >> 5) = 1u << (irq & 31u);
}

static inline uint32_t irq_save(void)
{
    uint32_t s;

    __asm volatile("mrs %0, primask\n cpsid i" : "=r"(s) : : "memory");
    return s;
}

static inline void irq_restore(uint32_t s)
{
    __asm volatile("msr primask, %0" : : "r"(s) : "memory");
}

static inline void sys_reset(void)
{
    SCB_AIRCR = 0x05FA0004u;    /* VECTKEY | SYSRESETREQ */
    for (;;)
        ;
}

#endif
