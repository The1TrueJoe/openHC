/* TM4C1231D5PM: 64 KB flash, 24 KB SRAM. The app starts at 0x1000; the TI
 * bootloader below it is never written. */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x00001000, LENGTH = 64K - 4K
  SRAM (rwx) : ORIGIN = 0x20000000, LENGTH = 24K
}
INCLUDE app.ld
