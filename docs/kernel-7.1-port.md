# Porting mainline Linux 7.1 to the Control4 IO Extender V1 (DM355 "hammer")

Status: scoping complete. This document is grounded in the vendor GPL source
(`vendor-gpl/`, esp. reconstructed `board-hammer.c`) and a file-by-file check of
the current mainline tree. It says exactly what must be built.

## TL;DR

A 7.1 port is **well-scoped, not open-ended**. Because the DaVinci `da850`
platform is still in mainline, *every peripheral driver we need already exists
and has DT bindings*. What was deleted is only the **DM355 SoC glue** (4 files),
which we resurrect from ≤v6.1 git history and forward-port into the still-living
clock/irq frameworks. The custom IO is almost all plain GPIO (no driver) plus one
small FPGA loader and one small IR-out driver.

Realistic effort for one embedded-Linux engineer with serial console:
**~4–6 weeks to Ethernet + all digital IO + 4 serial; +1–3 weeks for IR-out.**

## Mainline 7.1 gap analysis (verified against torvalds/master)

### Removed — must resurrect + forward-port (from a ≤ v6.1 checkout)
| File | Role |
|------|------|
| `arch/arm/mach-davinci/dm355.c` | SoC: device base addrs, clock wiring, pinmux, EDMA, timers |
| `arch/arm/mach-davinci/board-dm355-evm.c` | reference board (template for our board file) |
| `drivers/clk/davinci/psc-dm355.c` | CCF power-sleep-controller clock data |
| `drivers/clk/davinci/pll-dm355.c` | CCF PLL clock data |
| `drivers/irqchip/irq-davinci-aintc.c` | ARM AINTC irq controller (DM355 uses AINTC; da850 uses cp-intc) |

These slot into **frameworks that still exist** (the CCF `drivers/clk/davinci/`
and the davinci irqchips are alive for da850), so this is adaptation, not
from-scratch bring-up.

### Present in 7.1 — reuse as-is (DT or platform)
`arch/arm/mach-davinci/da850.c` (living reference), `gpio-davinci`,
`drivers/memory/ti-aemif.c`, `drivers/mtd/nand/raw/davinci_nand.c`,
`drivers/net/ethernet/davicom/dm9000.c`, `8250`/`8250_of`, `drivers/dma/ti/edma.c`,
`i2c-davinci`, `davinci_wdt`, `drivers/clk/davinci/psc-da850.c`, cp-intc.

## Board resource map (from reconstructed `board-hammer.c`)

Machine: `MACHINE_START(DAVINCI_DM355_EVM, "Control4 DM355 I/O Bar")`,
ATAGS boot params @ `0x80000100`, `davinci_timer`, AINTC irqs.

### Memory map
- SoC async EMIF control: `0x01e10000` (davinci_nand controller)
- NAND data (CE0): `0x02000000`, 32 MB window (Micron 512 MiB, 14 MTD parts)
- **FPGA on EMIF CE1: base `0x04000200`** (`DAVINCI_ASYNC_EMIF_DATA_CE1_BASE+0x200`, 0x100 window)
  - IR out 0/1: `+0x20` / `+0x30`
  - UART0..3: `+0x40 / +0x50 / +0x60 / +0x70` (each 0x10 → stock 16550A)
  - AC97 (unused): `+0x80`
  - FPGA core IRQ: GPIO(2), high-edge; FPGA serial IRQ: GPIO(7), high-edge
- Ethernet dm9000: `0x04014000` (addr) / `0x04014002` (data), IRQ `IRQ_DM355_GPIO1`,
  16-bit-only, no-EEPROM, simple-PHY; **reset via GPIO(101)**, link sense GPIO(1)

### GPIO map (SoC GPIO)
| Function | GPIO |
|---|---|
| Relays 1–8 | 88, 89, 90, 91, 92, 93, 94, 95 |
| Contacts 1–8 | 71, 70, 82, 83, 84, 85, 86, 87 |
| ID button / Recovery button | 9 / 8 |
| data-led / link-led | 75 / 76 |
| Status LEDs (orange/blue/red) | PWM0 / PWM1 / PWM2 |
| FPGA slave-serial: M2,M0,CCLK,DONE,DIN,INIT_B,PROG_B | 55, 57, 96, 97, 58, 7, 98 |

### I2C
davinci-i2c @ 400 kHz, bus 1: 24c08 EEPROM @ 0x50. (Temp sensor is behind the FPGA.)

## What we build

1. **SoC support (the core task)** — resurrect the 4 removed files at a v6.1 tag,
   fix them up to 7.1 APIs. Two routes:
   - **(A) Board-file / ATAGS (recommended for bring-up).** Keep DM355 as a legacy
     `DT_MACHINE`/board file + a `board-hammer.c` (we already have the 2.6.28 one as
     a spec). The stock Control4 U-Boot passes ATAGS natively → **no DTB, no
     bootloader changes**. Fastest path to first boot.
   - **(B) Device tree (clean, more work).** Author `dm355.dtsi` + `hammer.dts`.
     All peripheral drivers already have DT bindings; only the resurrected SoC code
     + AINTC need DT wiring. Boot via `CONFIG_ARM_APPENDED_DTB` (append DTB to the
     zImage, wrap as legacy uImage) so old U-Boot needs no FDT support.
   Recommend A first, migrate to B later if desired.

2. **Digital IO — no kernel driver.** Relays/contacts/buttons/LEDs are plain GPIO.
   Expose the gpiochip and drive from userspace with **libgpiod**, or describe them
   as `gpio-leds`/`gpio-keys` in DT. (This is also where the openHC IO daemon lives.)

3. **4× serial — stock 8250.** Register 4 ports at `0x04000240/250/260/270` sharing
   the GPIO(7) IRQ (board file: `serial8250_register_8250_port`; DT: 4 `ns16550a`
   nodes). Needs the FPGA UART input clock (recover from `c4serial.ko` or measure).

4. **FPGA loader.** (a) Configure AEMIF CS1 timing via `ti-aemif` so the window is
   accessible; (b) load `fpga_fw.bin` (we have it: `vendor-gpl` device copy) via
   **Xilinx slave-serial bit-bang** on the 7 GPIOs above. Standard protocol
   (pulse PROG_B, wait INIT_B, clock each bit on DIN/CCLK, wait DONE). Can be a tiny
   kernel driver *or* a userspace libgpiod tool. After load, the 8250 UARTs + IR-out
   windows are live.

5. **IR-out driver — the only bespoke driver.** Two 0x10 register windows
   (`0x04000220/230`). Rewrite from the on-device `c4irout.ko` (register map +
   carrier/timing) — small, self-contained.

## Boot & test loop (low brick risk)

- U-Boot `run tst` **RAM-netboots** a kernel via TFTP without touching flash — the
  entire Phase 1/2 iteration happens here. Set up a TFTP server, build uImage,
  `run tst`, repeat.
- Dual bank + recovery mean even a bad flash is recoverable. Only flash bank 0
  once the kernel is trusted; keep bank 1 (stock) + recovery intact.
- Serial console on `ttyS0 @115200` (SoC UART0) is required for kernel work —
  locate/solder the header (U-Boot + early kernel log land here).

## Staged plan

| Phase | Goal | Est. |
|-------|------|------|
| 0 | Serial console, TFTP netboot loop, v6.1 DM355 code checkout, `nanddump` backups | days |
| 1 | 7.1 boots to shell: AINTC + clocks + timer + **dm9000** + **davinci_nand**, NFS/JFFS2 root | 2–4 wk |
| 2 | Digital IO: gpiochip up → relays/contacts/buttons/LEDs via libgpiod | 2–4 days |
| 3 | FPGA: AEMIF timing + slave-serial loader + 4× 8250 UARTs | 1–3 wk |
| 4 | IR-out driver (reverse `c4irout.ko`) | 1–2 wk |

## Open unknowns to close (small)
- FPGA UART input clock frequency (for 8250 `uartclk`) — from `c4serial.ko`/measure.
- IR-out register semantics — from `c4irout.ko`.
- AINTC forward-port details to 7.1 irqchip API.
- Confirm ATAGS path still viable on 7.1 for this mach (vs mandatory DT).

## Notes on the GPL drop
Control4's published source (`vendor-gpl/`) includes the GPL kernel, U-Boot, the
board files (`board-hammer.c`, `board-c4davinci.c`), headers, and the U-Boot FPGA
loader — but **omits the `c4fpga.c`/`c4gpio.c`/`c4irout.c` driver bodies** (a
partial-compliance gap). Not a blocker: digital IO needs no driver, and the FPGA
loader protocol + register windows are documented in `board-hammer.c` and
recoverable from the on-device `.ko` binaries.
