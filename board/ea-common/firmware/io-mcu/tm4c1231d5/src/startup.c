/* Cortex-M4 vector table and reset path.
 *
 * The 4 KB TI serial bootloader owns flash 0x0000..0x0FFF and jumps to the
 * application at 0x1000, so THIS vector table must be linked there. Never emit
 * anything into the low 4 KB: leaving the bootloader intact is what makes a bad
 * application image recoverable over the wire instead of needing JTAG.
 */
#include <stdint.h>

extern uint32_t _sdata, _edata, _sidata, _sbss, _ebss, _estack;
int main(void);

static void default_handler(void)
{
    for (;;) {
    }
}

/* Vector Table Offset Register. */
#define SCB_VTOR (*(volatile uint32_t *)0xE000ED08u)

void reset_handler(void)
{
    /* Point the CPU at OUR table. The 4 KB TI bootloader contains no reference
     * to 0xE000ED08, so it does not relocate for us — without this every
     * exception and IRQ would vector through the bootloader's table. Harmless
     * while nothing interrupts; fatal the moment the IR timers do.
     */
    SCB_VTOR = 0x00001000u;

    uint32_t *src = &_sidata, *dst = &_sdata;
    while (dst < &_edata) {
        *dst++ = *src++;
    }
    for (dst = &_sbss; dst < &_ebss; dst++) {
        *dst = 0;
    }
    main();
    for (;;) {
    }
}

/* TM4C1231D5PM has IRQ 0..106, so a COMPLETE table is 16 system entries plus
 * 107 interrupts = 123. A 16-entry table leaves every peripheral interrupt
 * vectoring into whatever follows in flash — which is our own code, so it does
 * not fault, it does something arbitrary. Both IR paths are interrupt-driven,
 * so this has to be right before either is switched on.
 */
#define NUM_IRQS 107

__attribute__((section(".isr_vector"), used))
void (*const vector_table[16 + NUM_IRQS])(void) = {
    (void (*)(void))&_estack,
    reset_handler,
    default_handler,   /* NMI */
    default_handler,   /* HardFault */
    default_handler,   /* MemManage */
    default_handler,   /* BusFault */
    default_handler,   /* UsageFault */
    0, 0, 0, 0,
    default_handler,   /* SVCall */
    default_handler,   /* DebugMon */
    0,
    default_handler,   /* PendSV */
    default_handler,   /* SysTick */
    /* IRQ 0..106 — every one lands on default_handler until claimed. Explicit
     * rather than relying on zero-fill: a null vector faults unhelpfully, a real
     * handler that spins is at least findable with a debugger. */
    [16 ... 16 + NUM_IRQS - 1] = default_handler,
};
