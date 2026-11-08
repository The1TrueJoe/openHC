---
title: HC-250 bring-up
description: Booting openHC on an HC-250 from a USB stick, with no serial and nothing written to NAND.
sidebar:
  order: 2
  label: Bring-up
---

The HC-250 board lives in `board/hc250/`, standalone — it shares nothing with the
HC-800 but the IO microcontroller image, so there is no common HC folder.

## The way in: a RAM boot from a USB stick

Stock U-Boot runs `c4test.img` off a FAT USB stick when the ID button is held at
power-on (the boot chain is on the [Overview](/hc250/)).
The image build produces both files:

| File | What |
|---|---|
| `openhc-hc250-kernel.img` | legacy uImage: zImage + appended DTB + the whole rootfs as an initramfs |
| `c4test.img` | U-Boot script: `fatload usb 0:1 0x82000000 openhc-hc250-kernel.img; bootm` |

1. Copy both onto a FAT-formatted USB stick and plug it into the rear USB port.
2. Hold the **ID** button, apply power, and **let go after about two seconds**.
   Never hold it for seven: that is Control4's Clean Start, which erases the
   configuration partitions.
3. openHC comes up from RAM and takes DHCP on eth0 with the unit's own MAC.

Nothing is written anywhere. The script sets `ohc.ramboot=1`, so `/init` does
not even look at the SD card. Power-cycling without the button boots stock. If
the stick or the image is missing, the script fails and U-Boot carries on into
the stock boot.

From a running openHC, the next kernel is `kexec`, not another button press.

## What the first image is

- **Kernel:** mainline 7.1.8, `omap2plus_defconfig` + `linux/hc250.fragment`.
- **Device tree:** `linux/c4-omap3-hc250.dts`, on `ti/omap/omap36xx.dtsi`.
  Stock U-Boot passes ATAGS, so the DTB is appended to the zImage
  (`ARM_APPENDED_DTB` + `ARM_ATAG_DTB_COMPAT`).
- **Pin mux is U-Boot's.** The DTS has no board pinctrl yet; every pad stays as
  stock U-Boot left it. The stock mux table was saved off the unit for when that
  changes.
- **MAC:** `smsc911x` reads its address only from DT properties, which this
  U-Boot can't fill. `S05ethaddr` sets it from `eth=` (the script passes U-Boot's
  `${ethaddr}`), falling back to the copy in the board EEPROM at offset 8.
- **IO:** the `iomcu` feature. The IO-MCU node describes block 1 — four jacks,
  blaster, receiver, one relay, one contact — so `gpio-ohc-iomcu` names the lines
  and lirc devices exactly as on the HC-800. No `firmware-name` yet: it talks to
  Control4's firmware until ours is proven on this board.
- **NAND:** described with every partition `read-only`.
- **Held lines:** `zigbee_reset` and `zigrf_enable` high, the 12 V enable high,
  and the Wi-Fi radio held in reset, all as stock leaves them.

Not in the first image: Zigbee (the feature brings Node, too big for a RAM
image), Wi-Fi (MMC2 disabled), video, audio.

## Kernel patches

**None for the first boot.** Everything the board needs is mainline:

| Block | Mainline |
|---|---|
| OMAP36xx, GPMC, UART, I2C, MMC, McBSP, watchdog | `omap2plus` |
| TPS65950 (power, GPIO, RTC, USB PHY, MADC, codec) | `twl4030` drivers |
| LAN9220 | `smsc911x` |
| NAND | `omap2` raw NAND, BCH |
| MUSB host | `musb` + `twl4030-usb` |
| ADV7513 | `adv7511` DRM bridge |
| EEPROM | `at24` |

What may still need one:

- **THS8200** (component out). No mainline driver. The HC-800 drives it from
  userspace over I2C (`ths8200` package); that may carry over.
- **Control4's NAND ECC layout.** If its BCH4 OOB layout differs from mainline's,
  reading stock partitions without errors needs a layout patch. Only matters for
  reading stock data or installing to NAND.
- **Audio** needs no driver patch: the TPS65950 codec is `ti,omap-twl4030` and
  HDMI audio is McBSP4 into the ADV7513, both expressible in the DT. Control4's
  `magneto` machine driver is not needed.
- **Wi-Fi** depends on the module: `brcmfmac` if it is a supported Broadcom SDIO
  part.

The one openHC driver change for this board was in our own `gpio-ohc-iomcu`: a
combo jack may now have a single serial-enable line (the HC-250's PB0/PB1), not
only the EA's pair.

## Next

1. First RAM boot: serial console, Ethernet, USB, the IO-MCU answering.
2. Find the interior SD card; a persistent rootfs there (Zigbee/Node can live on it).
3. TPS65950 audio (codec DAC/ADC) and the MADC.
4. Video: DPI → ADV7513, then the THS8200.
5. Wi-Fi on MMC2.
6. openHC's own LM3S1162 firmware on this board's pins.
