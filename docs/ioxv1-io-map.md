# IO Extender V1 (DM355 "hammer") — IO map

Authoritative map of the on-board IO for openHC, from the DM355 TRM (sprufb3),
the vendor `board-hammer.c`, and live bring-up. GIO*n* = DM355 GPIO *n* =
`gpiochip0` line *n* (libgpiod). All confirmed on hardware unless noted.

## Pin mux (must be set before any of the GPIO IO below works)

These pins are shared with the (unused) camera/video ports and U-Boot leaves
them muxed to video. `board/ioxv1/rootfs-overlay/etc/init.d/S01pinmux` flips them
to GPIO at boot:

| Reg | Addr | Write | Effect |
|----|----|----|----|
| PINMUX0 | 0x01c40000 | `0x00007955` | GIO86-95 → GPIO (relays 88-95, contacts 86/87) |
| PINMUX1 | 0x01c40004 | `0x0014416A` | GIO75/76 → GPIO (data/link LEDs); keeps PWM0/1/2 |

TODO (same mechanism, not yet done): contacts GIO70/71 (PINMUX1 bits 17-19) and
GIO82-85 (PINMUX0 bits 11-14).

## LEDs

| Name | Type | How to drive | Notes |
|----|----|----|----|
| **data** (front) | GPIO | `gpiochip0` line **75**, active-high | 1 = on |
| **link** (front) | GPIO | `gpiochip0` line **76**, active-high | 1 = on |
| **status** (front) | PWM0 @ 0x01c22000 | orange | blink = slow period; steady = small PER |
| **power** (front) | PWM1 @ 0x01c22400 | blue | |
| (red) | PWM2 @ 0x01c22800 | red | part of the tri-color status |
| rear status/data/link/power | **TBD** | — | not in the vendor board file; needs discovery |

PWM registers (base + ch*0x400): PCR +0x04, CFG +0x08, START +0x0c, RPT +0x10,
PER +0x14 (period), PH1D +0x18 (phase-1 / duty). LEDs are active-low. A ~1 s PER
reads as a blink; a small PER (e.g. 0x1000) with PH1D≈PER/2 is a steady glow.

## Relays (8) — CONFIRMED clicking

`gpiochip0` lines **88-95** (relay1..relay8), active-high (1 = energized).
Direct SoC GPIO. Register bank for GIO64-95 is at `0x01c67060` (DIR+0, OUT+4,
SET+8, CLR+0xc, IN+0x10; bit = gpio%32; relays = bits 24-31).

## Contacts (8, inputs)

`gpiochip0` lines 70, 71, 82, 83, 84, 85, 86, 87. 86/87 already GPIO (PINMUX0
fix); 70/71/82-85 still need pinmux (see TODO above).

## Ethernet — working

dm9000 → `eth0`. IRQ = GIO1 (rising), reset = GIO101, MAC forced 00:0f:ff:18:21:9c.

## Serial

| Port | Device | Backing | Status |
|----|----|----|----|
| debug console | ttyS0 | SoC UART0 @ 0x01c20000 | working (RX only unless TX wired) |
| RS232 1-4 | ttyS1-4 | **FPGA UARTs** @ 0x04000240/250/260/270 | BLOCKED: FPGA not programmed |

RS232 needs the FPGA loaded (`fpga_fw.bin`, in stock NAND) via the `c4fpga`
bit-bang loader, then ns16550a DT nodes (shared IRQ on GIO7). CE1 (0x04000000+)
is **not** readable from userspace `devmem` — needs a kernel driver.

## IR-out (8) — FPGA

FPGA @ 0x04000220 / 0x04000230. Also needs the FPGA programmed.
