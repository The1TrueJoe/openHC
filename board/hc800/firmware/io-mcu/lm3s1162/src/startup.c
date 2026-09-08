/* Cortex-M3 vector table and reset path for the LM3S1162.
 *
 * Control4's 4 KB serial bootloader owns flash 0x0000..0x0FFF and jumps to the
 * application at 0x1000, so THIS vector table must be linked there. Never emit
 * anything into the low 4 KB: leaving the bootloader intact is what makes a bad
 * application image recoverable over the wire instead of needing SWD.
 */
#include <stdint.h>

#include "lm3s.h"

extern uint32_t _sdata, _edata, _sidata, _sbss, _ebss, _estack;
int main(void);

/* Latched at reset, before anything can disturb SYSCTL, and served by main()'s
 * identity reply. This is how we find out what the part actually is: DC0 gives
 * the real flash and SRAM sizes, and lm3s.ld has to assume a conservative SRAM
 * because nothing in the vendor images reveals it. See the note in lm3s.ld. */
uint32_t g_dc0;
uint32_t g_did1;

static void default_handler(void)
{
    for (;;) {
    }
}

void reset_handler(void)
{
    /* Point the CPU at OUR table. The bootloader's table lives at 0x0000 and it
     * does not relocate for us, so without this every exception vectors through
     * the bootloader — harmless while nothing interrupts, fatal the moment
     * anything does. Same reasoning as the EA firmware, same one line. */
    SCB_VTOR = 0x00001000u;

    g_dc0  = SYSCTL_DC0;
    g_did1 = SYSCTL_DID1;

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

/* How long is a complete table on this part?
 *
 * The stock image answers for the vendor: its table is 46 entries — 16 system
 * exceptions plus 30 IRQs — which is the Stellaris Sandstorm count. That makes
 * sense for an image whose strings advertise LM3S615/811/815 alongside the
 * LM3S1162: it is built to the smallest member's interrupt map.
 *
 * We only claim the LM3S1162, and Fury-class parts carry more than 30, so a
 * 30-entry table would leave any higher interrupt vectoring into whatever
 * follows in flash — which is our own code, so it would not fault, it would do
 * something arbitrary. Sizing UP is free: every unclaimed slot is 4 bytes of
 * flash pointing at a handler that spins, which is at least findable with a
 * debugger. 46 IRQs is a safe superset of every Fury variant.
 *
 * Note this firmware currently takes NO interrupts at all — the UART and the
 * burst timer are both polled, exactly as the EA firmware does it. The table
 * matters anyway, for faults and for the day IR capture is wired up.
 */
#define NUM_IRQS 46

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
    [16 ... 16 + NUM_IRQS - 1] = default_handler,
};
