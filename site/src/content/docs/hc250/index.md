---
title: HC-250
description: A live pull off a stock HC-250 rev 8 — an OMAP36xx built from TI's OMAP3 EVM, with an LM3S1162 IO-MCU and a USB-stick way in.
sidebar:
  order: 1
  label: Overview & recon
---

Everything below came off a running **stock HC-250 (board revision 8)** over SSH
as root, plus Control4's own GPL U-Boot and X-Loader patches. Nothing on the unit
was written or reset.

The HC-250 is **not** a smaller HC-800. The HC-800 is an x86 PC; the HC-250 is a
TI **OMAP36xx** ARM board that Control4 derived from TI's OMAP3 EVM — its U-Boot
still sets `MACH_TYPE_OMAP3EVM`, and the Ethernet, PMIC and MMC wiring follow the
EVM. The only thing the two share is the IO microcontroller image.

## Identity

```
/proc/cpuinfo Hardware      HC250
/proc/cpuinfo Revision      0008
hostname                    home-controller-hc250-000FFF1AA0B1
uname                       Linux 2.6.32-51.48 #51.48-hc250.1 (Oct 12 2018) armv7l
OS                          2.10.6.561784
U-Boot                      2011.03-31.25 (Control4)
eth0 MAC                    00:0F:FF:1A:A0:B1
die id (U-Boot env)         1ba800029ff800000156083e01026025
```

**Board codename: `magneto`.** U-Boot's `imagepath`, the ALSA card name and the
GPL board directory (`board/c4/magneto`) all use it.

Root SSH works with the vendor password. The dropbear is 2016-era and offers only
`ssh-rsa` host keys, so a modern client needs `-o HostKeyAlgorithms=+ssh-rsa`.

## SoC and memory

```
ARMv7 Processor rev 2 (v7l), BogoMIPS 996.74
CPU part 0xc08 (Cortex-A8), variant 0x3, revision 2  -> r3p2
Features: swp half thumb fastmult vfp edsp neon vfpv3
mpurate=1000, DSPLink + CMEM loaded (C64x DSP present)
MemTotal 503044 kB (mem=504M; CMEM at 0x9f800000 for the DSP)
```

The vendor kernel is built `CONFIG_ARCH_OMAP3430`, which says nothing about the
silicon. The core does: **Cortex-A8 r3p2 is the OMAP36xx / DM37xx core** (the
OMAP35xx has r1p2/r1p3), it runs at **1 GHz** (beyond any OMAP35xx), and it has
the DSP. So: **DM3730 / OMAP3630 class.** The exact part needs `CONTROL_IDCODE`
(`md 4830a204 1` in U-Boot); `/dev/mem` refuses IO reads on this kernel and the
boot log scrolls out of the ring buffer within seconds (see NAND below).

Companion: **TPS65950** (TWL4030 family) on I2C1 @0x48–0x4b — power, RTC, USB
PHY, audio codec, MADC, 18 GPIOs.

## Storage

**512 MiB NAND**, 2 KiB pages, 128 KiB blocks, 64-byte OOB, 16-bit bus on GPMC
CS0, **BCH4** ECC. Stock U-Boot also keeps an on-flash bad-block table in the last
four blocks.

| Offset | Size | Stock MTD | What |
|---|---|---|---|
| `0x00000000` | 512 KiB | mtd7 | X-Loader, 3 copies (hardware ECC) |
| `0x00080000` | 2 MiB | mtd1 | U-Boot, 3 copies |
| `0x00280000` | 256 KiB | mtd6 | U-Boot environment |
| `0x002c0000` | 4 MiB | mtd2 | kernel 0 |
| `0x006c0000` | 128 MiB | mtd3 | rootfs 0 (JFFS2) |
| `0x086c0000` | 4 MiB | mtd11 | recovery kernel |
| `0x08ac0000` | 128 MiB | mtd12 | recovery rootfs |
| `0x10ac0000` | 24 MiB | mtd4 | `/mnt/jffs2` |
| `0x122c0000` | 80.4 MiB | mtd5 | `/mnt/internal` |
| `0x17320000` | 4 MiB | mtd9 | kernel 1 |
| `0x17720000` | 128 MiB | mtd8 | rootfs 1 |
| `0x1f720000` | 8 MiB | mtd10 | persistent logs |
| `0x1ff20000` | 256 KiB | mtd13 | redundant environment |

A/B systems (`bootsystem=0` on this unit) plus a recovery pair that factory
restore copies over the active one.

**The NAND is worn.** Every read of the rootfs logs a stream of corrected
single-bit errors — the same bits each time, so they are stuck cells, not read
disturb, and BCH4 still corrects them all. Treat it as fragile: openHC does not
write it. A full ECC-corrected dump (no OOB) was taken before anything else.

**There is also an 8 GB SD card in an interior slot.** The stock OS never sees it:
both MMC hosts register but neither enumerates a card. See MMC below for why it is
probably on MMC1.

## Boot chain: ROM → X-Loader → U-Boot 2011.03 → uImage

The vendor kernel takes a legacy uImage and ATAGS; nothing is signed. The U-Boot
environment (readable with `fw_printenv`) is the interesting part:

```
bootcmd   = if itest ${usebootcounters} -eq 1; then c4sys init; fi;
            if itest ${usewatchdogtimer} -eq 1; then watchdog set 300; fi;
            run boot${bootmode}                     # bootmode=nand
preboot   = if c4sys usbcheck; then
                if usb start && run loadscript; then run bootscript; fi;
            fi; c4sys restorecheck; c4sys ethaddrcheck; ...
loadscript = fatload usb 0:1 ${imgaddr} c4test.img  # imgaddr=8B000000
bootscript = echo Running USB Script...; source ${imgaddr}
bootmmc   = ... fatload mmc 1 ${bootload} uImage; bootm ${bootload}
```

**A FAT USB stick holding a U-Boot script called `c4test.img` is run before
anything else** — before the boot counters (`c4sys init`) and before the 300 s
watchdog is armed. That is the no-serial way in; see [Bring-up](/hc250/bring-up/).

What gates it, from Control4's GPL `0021-uboot-mfg-test-usb-support.patch`:

- `c4sys usbcheck` is true only when the **ID button is held at power-on** (board
  revision 6 or later; this is 8). Held for a second, it also switches on the rear
  USB 5 V (TWL GPIO 7 → TPS2553).
- **Factory restore is a different input** — OMAP GPIO 23, active-high, held 4 s
  through `restorecheck`. Holding ID cannot trigger it.
- **Holding ID for 7 s into `c4sys init` is "Clean Start"**: it erases the
  `jffs2` and `internal` partitions (the controller's configuration). Release ID
  after a couple of seconds.

## Peripherals as the stock kernel wires them

From `/proc/iomem`, `/sys`, and the GPIO table under debugfs:

| What | Where |
|---|---|
| Console | UART1 (`ttyS0`, `0x4806a000`), 115200 |
| Zigbee EM357 | UART2 (`ttyS1`, `/dev/ttySZigbee`), on the **McBSP3 pads** with RTS/CTS; `zigbee_reset` = GPIO 24, `zigrf_enable` = GPIO 25 |
| IO-MCU LM3S1162 | UART3 (`ttyS2`, `/dev/ttySStellaris`), 115200; `io_reset` = GPIO 96 |
| Ethernet | **LAN9220** (smsc911x) on GPMC CS5 @`0x2c000000`, IRQ GPIO 27 level-low, reset GPIO 177 |
| I2C1 (2.6 MHz) | TPS65950 @48–4b, 24C02-class EEPROM @50 (c4sys boot counters at 0–5, MAC at 8) |
| I2C2 (400 kHz) | THS8200 @21, ADV7513 @39 (IRQ GPIO 19) |
| MMC1 | 4-bit, card detect on TWL GPIO 0 (reads empty) — the interior SD, most likely |
| MMC2 | Wi-Fi SDIO behind a level shifter (`sdmmc2_dat4-7` as direction lines); `wifi_reset` = GPIO 176 (held low), `wifi_carddetect` = GPIO 101 (driven low), CD in = GPIO 94 |
| USB | MUSB OTG through the TPS65950 PHY; 5 V switch on TWL GPIO 7, its fault on TWL GPIO 6 |
| 12 V | GPIO 104 driven high, fault input GPIO 110 (vendor `tps2533` driver) |
| Buttons | ID = GPIO 22 (active-low, `gpio-keys`) |
| LEDs | power GPIO 111, data 178, link 182, Wi-Fi red/yellow/blue 179/180/181 — **all active-low** |
| Audio | TPS65950 codec (card `magneto`: DAC2 → headset/line out, TX1 analog in) + HDMI audio over McBSP4 (card `magneto-hdmi`) |
| Video | DSS DPI → ADV7513 (HDMI) and THS8200 (component), `omapfb.mode=dvi:1280x720-24@60` |

The Wi-Fi driver is Broadcom's `dhd.ko` (FullMAC SDIO, with its firmware and nvram in `/mnt/jffs2/dhd`), never
loaded on this unit: the radio is held in reset, which is why MMC2 is empty.

## IO: the LM3S1162, block 1

Same part and the **same `IoProcMultiConfig1162` image** as the HC-800; the board
strap selects **config block 1** (inferred — the strap voltage is unmeasured, but
block 1 matches the HC-250's I/O exactly). Decoded with the HC-800's layout; full
table in [IO-MCU pin map](/shared/io-mcu-pinmap/):

- four rear IR jacks and a front blaster, gated onto two PWM carriers (PD1, PF3)
  by the same pins as HC-800 jacks 1–4;
- front IR receiver on PD4;
- **one relay (PE0), one contact (PE1)**;
- two MCU serial ports, UART1 (PD2/PD3) and UART2 (PG0/PG1), each with **one**
  serial-enable line (PB0, PB1) — jacks 1 and 2 are the combo IR/serial jacks.

## Control4's GPL source

From `http://update.control4.com/open_src/2.9.0.525559-res/src/` (the index is
`src-2.9.0.525559-res.xml` in that directory; the 2.10.6 directory 403s):

- `u-boot-2011.03+patches.tar.gz` — **the magneto board**, its pin mux
  (`0007-uboot-magneto-mux-config`), c4sys (counters, restore, `usbcheck`), the
  EEPROM layout, BCH4 NAND.
- `x-load-omap3-1.51.tgz+patches.tar.gz` — magneto X-Loader.
- **No magneto kernel.** `linux-omap-2.6.32+patches` in the same drop is the
  "Badger" touchscreen platform. The HC-250's board file and its `magneto` ASoC
  machine driver are not published, which is why the pin map above came off a
  live unit.

## Open questions

- Exact SoC (`CONTROL_IDCODE`) and whether it is a GP or HS part.
- Which MMC the interior SD card is on, and why stock never sees it.
- The Wi-Fi module (RF can marked `GB9619 T10F4010 1524`) and its Broadcom chip.
- Control4's BCH4 OOB layout versus mainline's — needed before the kernel reads
  NAND without ECC errors.
- The IO-MCU strap voltage, and which MCU UART is behind which combo jack.
