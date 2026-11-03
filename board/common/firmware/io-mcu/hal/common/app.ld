/* Sections, shared by both parts; each part's link.ld gives MEMORY. */
_estack = ORIGIN(SRAM) + LENGTH(SRAM);
SECTIONS
{
  .text : { KEEP(*(.isr_vector)) *(.text*) *(.rodata*) . = ALIGN(4); } > FLASH
  _sidata = LOADADDR(.data);
  .data : { _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > SRAM AT > FLASH
  .bss  : { _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > SRAM
  _stack_floor = _ebss + 1024;
  ASSERT(_stack_floor <= _estack, "less than 1 KB left for the stack")
  /DISCARD/ : { *(.ARM.exidx*) *(.comment) }
}
