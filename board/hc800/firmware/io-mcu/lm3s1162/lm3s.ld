/* LM3S1162 memory map.
 *
 * 64 KB flash. The application starts at 0x1000 — the low 4 KB is Control4's
 * serial bootloader (IRBootloaderSerialLM3S1162.bin, 4,068 bytes), which must
 * never be overwritten because it is the only recovery path that does not need
 * SWD. Both numbers are FACTS read out of the vendor images, not datasheet
 * lookups:
 *
 *   - the stock application file is 65,794 bytes = 256-byte Control4 header +
 *     a 65,536-byte flash image + a 2-byte CRC, so flash is 0x0000..0xFFFF;
 *   - inside that image the low 4 KB is left 0xFF and a valid Cortex-M vector
 *     table appears at flash 0x1000 (SP 0x2000051C, reset 0x00007CE8);
 *   - the bootloader's own table sits at flash 0x0000 (SP 0x20000C40,
 *     reset 0x0000007C).
 *
 * ── SRAM LENGTH IS DELIBERATELY CONSERVATIVE, AND HERE IS WHY ─────────────
 *
 * We do not know this part's SRAM size, and the EA's linker script is right
 * that guessing it is not a warning but a dead board: an _estack above real
 * SRAM hard-faults on the first push, before anything can report it.
 *
 * The vendor images do NOT tell us. Their initial SPs (0x2000051C, 0x20000C40)
 * are low because TI's StellarisWare startup_gcc.c declares the stack as a
 * static array in .bss and points the vector at its end — so those values
 * locate a linker section, not the top of memory.
 *
 * So: 8 KB, which is safe on any LM3S1xxx. Being LOW is merely wasteful; being
 * HIGH is fatal. And the waste costs us nothing real — this firmware's static
 * footprint is the protocol buffers and the IR job, low single-digit KB — so
 * there is no reason to reach for the rest even if it is there.
 *
 * We therefore do NOT try to relocate the stack at runtime. An earlier draft
 * had reset_handler read DC0 and raise MSP; that trades a guaranteed-safe
 * constant for a stack switch executed from inside a C function whose own frame
 * is on the stack being switched. Not worth it for RAM we do not want.
 *
 * Instead the firmware REPORTS the truth: startup.c latches SYSCTL DC0 and
 * main.c serves the decoded flash/SRAM sizes, so the first boot tells us what
 * the part really is. If that says more than 8 KB and we ever need it, raise
 * the constant here — with a measurement behind it.
 */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x00001000, LENGTH = 64K - 4K
  SRAM (rwx) : ORIGIN = 0x20000000, LENGTH = 8K
}
_estack = ORIGIN(SRAM) + LENGTH(SRAM);
_sram_base = ORIGIN(SRAM);
SECTIONS
{
  .text : { KEEP(*(.isr_vector)) *(.text*) *(.rodata*) } > FLASH
  _sidata = LOADADDR(.data);
  .data : { _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > SRAM AT > FLASH
  .bss  : { _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > SRAM
}
