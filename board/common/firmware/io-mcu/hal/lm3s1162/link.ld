/* LM3S1162: 64 KB flash, 16 KB SRAM. The app starts at 0x1000; the
 * bootloader below it is never written. */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x00001000, LENGTH = 64K - 4K
  SRAM (rwx) : ORIGIN = 0x20000000, LENGTH = 16K
}
INCLUDE app.ld
