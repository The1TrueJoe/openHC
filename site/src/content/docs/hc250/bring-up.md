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

## The other way in: kexec from stock, over SSH

The stock kernel (2.6.32) has `CONFIG_KEXEC` off, but it loads modules, has
`/proc/kallsyms`, and ships `/proc/config.gz`. `board/hc250/stock-kexec/`
adds the missing piece as a module, so openHC boots from a stock unit with
only SSH: no button, no stick, no serial. Nothing is written to flash; a reboot
or power cycle comes back up in stock.

| File | What |
|---|---|
| `ohc_kexec.c` | the module: `/dev/ohc-kexec` takes the image, `go` boots it |
| `ohc-kexec.sh` | runs on the unit: insmod, load the image, boot (or `dry`) |
| `build.sh` | builds `ohc_kexec.ko` with no kbuild (below) |
| `stock-kernel.config` | the unit's `/proc/config.gz` |
| `stock-crcs.txt` | export CRCs from the stock modules' `__versions` |

```sh
board/hc250/stock-kexec/build.sh          # -> board/hc250/stock-kexec/out/
scp -O out/ohc_kexec.ko out/ohc-kexec.sh root@<unit>:/tmp/
# on the unit:
/tmp/ohc-kexec.sh http://<host>/openhc-hc250-kernel.img dry   # plan only, stays in stock
/tmp/ohc-kexec.sh http://<host>/openhc-hc250-kernel.img       # boot it
```

**Building against a kernel with no source.** Control4 never published the
`magneto` kernel, so the module is built against the closest GPL tree (the
2.9.0 drop's `linux-omap-2.6.32`) configured with the unit's own config.
`build.sh` does by hand what kbuild and modpost would: it generates
`autoconf.h`, `bounds.h` and the version headers, compiles with gcc 4-era
flags (`-std=gnu89`, ARM-only relocations, no unwind tables), and writes the
`.mod.c` with `__versions` taken from the stock modules. The result matches the
stock modules where the loader checks: the same `vermagic`
(`2.6.32-51.48 mod_unload modversions ARMv7`), `struct module` the same size
(0x144) with `init`/`exit` at the same offsets, and only `ABS32`/`CALL`/`JUMP24`
relocations. The module imports only symbols the stock modules already import;
everything else (`__get_free_pages`, `kernel_restart_prepare`,
`setup_mm_for_reboot`, `v7_flush_kern_cache_all`) it looks up through
`kallsyms_lookup_name`, whose address the script passes in from
`/proc/kallsyms`.

**What `go` does.**

1. The image arrives in page chunks (1 MiB when free) that lie outside the RAM
   it will be copied to, `0x80000000` to `load_addr` + 48 MiB. A uImage header
   is skipped, and anything that is not a zImage is refused.
2. It builds ATAGs at `0x80000100`: 504 MiB of memory, like stock's
   `mem=504M`, which keeps the DSP carve-out untouched, and the command line.
   The decompressor (`ATAG_DTB_COMPAT`) folds them into the appended DTB.
3. It stops WDT2, which stock's `watchdogd` holds open with `NOWAYOUT`, and then
   calls `kernel_restart_prepare`, the reboot path's notifiers and
   `device_shutdown`.
4. With IRQs and FIQs off, it maps RAM 1:1 (`setup_mm_for_reboot`), cleans and
   invalidates every cache level, and jumps to a stub at its physical address.
5. The stub turns the MMU and caches off, copies the chunks to `0x82000000` and
   the ATAGs into place, and enters the zImage the way U-Boot's `bootm` does.

`echo dry > /sys/module/ohc_kexec/parameters/go` does steps 1 and 2 and prints
the copy plan to `dmesg` without leaving stock. `shutdown=0` skips
`device_shutdown`, if a stock driver hangs in it.

## What the first image is

- **Kernel:** mainline 7.1.8, `omap2plus_defconfig` + `linux/hc250.fragment`.
- **Device tree:** `linux/dts/ti/omap/c4-omap3-hc250.dts`, on `omap36xx.dtsi`. It sits
  under `ti/omap/` like the in-tree boards, because `omap36xx.dtsi` pulls in its clock
  files by bare name.
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

1. First RAM boot (USB stick or kexec from stock): serial console, Ethernet, USB,
   the IO-MCU answering.
2. Find the interior SD card; a persistent rootfs there (Zigbee/Node can live on it).
3. TPS65950 audio (codec DAC/ADC) and the MADC.
4. Video: DPI → ADV7513, then the THS8200.
5. Wi-Fi on MMC2.
6. openHC's own LM3S1162 firmware on this board's pins.
