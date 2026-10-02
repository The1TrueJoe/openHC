/* TM4C1231D5PM memory map.
 *
 * 64 KB flash, 24 KB SRAM — NOT the 256 KB/32 KB of the larger TM4C123 parts.
 * Verified two ways: the stock application image is exactly 65536 bytes and
 * spans 0x0000..0xFFFF, and its vector table's initial SP is 0x2000560C, which
 * is inside 24 KB (top 0x20006000) but would be nonsense for a 12 KB part.
 * libopencm3's device DB agrees: `tm4c123?d5* ROM=64K RAM=24K`.
 *
 * Getting this wrong is not a warning, it is a dead board: an _estack above the
 * real top of SRAM hard-faults on the first push, before anything can report it.
 *
 * The application starts at 0x1000 — the low 4 KB is the TI serial bootloader,
 * which must never be overwritten because it is the only recovery path.
 */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x00001000, LENGTH = 64K - 4K
  SRAM (rwx) : ORIGIN = 0x20000000, LENGTH = 24K
}
_estack = ORIGIN(SRAM) + LENGTH(SRAM);
SECTIONS
{
  .text : { KEEP(*(.isr_vector)) *(.text*) *(.rodata*) } > FLASH
  _sidata = LOADADDR(.data);
  .data : { _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > SRAM AT > FLASH
  .bss  : { _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > SRAM
}
