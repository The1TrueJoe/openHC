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

## First boot (2026-11-08)

Kexec from stock works: stock to stock, then stock to openHC (Linux 7.1.8) in
about 30 s, back on the same DHCP address. `reboot` in openHC comes back up in
stock. No serial, no stick, nothing written.

| Block | Under openHC |
|---|---|
| Ethernet | up, MAC from `eth=` |
| Interior SD | `mmcblk0`, 8 GB: stock's `Internal_896M` and `Apps` partitions |
| NAND | all 13 partitions, read-only |
| IO-MCU | Control4 03.26.15: relay 1, contact 1, IR out 1–4, blaster, receiver, 2 MCU serial ports |
| Zigbee (EM357) | answers on `ttyS1`, see below |
| TPS65950 | RTC, GPIO, USB PHY, MADC (`iio:device0`, 16 channels) |
| I2C 2 | THS8200 at `0x21`, ADV7513 at `0x39` (maps at `0x38`/`0x3c`/`0x3f`), unknown at `0x4e` |
| LEDs, watchdogs | all six LEDs, both watchdogs |
| USB host | root hubs up; no device tried |

Two things stock leaves behind:

- **The IO-MCU app is mute.** Stock's `ioserver` runs the MCU's UART at
  460800, and after the hand-off the app answers nothing at any rate.
  `gpio-ohc-iomcu` now pulses `reset-gpios` when its first knock goes unanswered;
  the part comes up in the TI bootloader and the driver starts the app as usual.
- **The EM357 sits in its serial bootloader** (`EM357 Serial Bootloader v45 bA8`,
  menu on `\r`). `2` (run) starts the EZSP app, which then answers an ASH reset
  with `RSTACK`. Whether stock's shutdown or our `zigbee_reset` hog puts it there
  is not known yet.

Noise in `dmesg`, none of it blocking: `twl4030_keypad` (no keymap; the board has
no keypad), `omap_ssi_port` (-2), `ti-sysc 48318000` (-16), and one L3 in-band
error early in boot.

MADC raw values on an idle unit: ch2 257, ch6 154, ch7 567, ch8 761, ch12 572,
the rest near 0. Which inputs are wired to what is not mapped yet.

## Installing to the SD card

openHC boots from the interior SD card through stock U-Boot, falls back to stock
on its own, and a factory restore returns the unit to stock. Proven 2026-11-08:
openHC up 40 s after `reboot`, with `ohc.boot=sd` on the command line.

**How it boots.** Stock `bootcmd` ends in `run boot${bootmode}` (`bootmode=nand`).
The install sets `bootmode=ohc` and adds `bootohc`:

1. Read EEPROM byte 6 (`i2c read 50 6 1`). At 3 or more (or an unarmed `0xff`),
   boot stock (`bootnand`).
2. Count the attempt (`i2c mw 50 6 <n+1>`).
3. `fatload` `openhc.img` from SD partition 3 (`mmc rescan 0`, then `1`) and
   `bootm` it. Anything failing falls through to `bootnand`.

`S95ohcboot` zeroes byte 6 once eth0 has an address (and reboots after five
minutes without one, so the count keeps climbing). It also zeroes stock's own
`c4sys init` counters (bytes 0/1, both slots, and the clean count in byte 2), as
stock's `setbootcnt` does after a good boot. Not via the env's `bootsystem`: after
stock's writes, Linux reads both env copies as bad CRC (bit flips only U-Boot's
ECC corrects). Left alone, U-Boot
switches stock to its other slot after three boots, then stalls at "All
Bootsystems Failed".

**Remote factory restore** (proven 2026-11-08). `restore stock` on openHC puts
Control4's IO firmware back (below), writes `R` to EEPROM byte 7 and reboots. `bootohc` sees it, clears it, zeroes Control4's
boot counters as the button path does, and runs `restoresystem`. About 4½ minutes
later the unit answered as stock: the recovery image (Control4 2.5.x, kernel
2.6.32-47.41, dropbear 0.52), `bootmode=nand`, `c4server`/`ioserver` running.
Running `ohc-boot-env.sh install` on that stock and rebooting brought openHC back
from the SD card, which the restore does not touch.

**The IO controller goes back too.** The restored OS does not rewrite the MCU (it
ran five minutes on openHC's image without touching it), and openHC cannot read
the stock rootfs on the NAND. So the install keeps stock's `/control4/firmware/io`
tree on `ohcdata`:

```sh
ssh <stock> /tmp/ohc-boot-env.sh stock-io | ssh <openhc> ohc-sd-install stock-io
```

`restore stock` writes Control4's image back first (the one `.flash.config` names
for `hc250`, through the driver's firmware upload, 12 s), and only then arms the
restore; without a kept tree it refuses unless `--keep-iomcu`. Proven 2026-11-08:
03.17.05 written over openHC's image, restore, stock's `ioserver` up on it, and on
the next openHC boot the driver found 03.17.05 and wrote its own image back.

**Factory restore.** The restore button runs the env script `restoresystem`
(recovery kernel and rootfs into slot 0, then `cleanboot`). The install appends
`setenv bootmode nand; saveenv`, so U-Boot itself puts the env back on stock.
Stock 2.10.6 never reads the SD card, so the card's layout does not matter to it.

**Why openHC never writes the NAND.** Mainline reads the env byte-for-byte equal
to a NAND dump, but with 256 "ECC unrecoverable" per partition. Control4's BCH4
keeps its 4×7 ECC bytes at OOB 36–63; mainline's omap2 BCH4 layout puts them at
2/10/18/26. A write from Linux would read back as uncorrectable in U-Boot. So
every partition stays `read-only`, the try count lives in the EEPROM, and the env
is written once at install by stock's own `fw_setenv`.

**SD layout:**

| | Size | What |
|---|---|---|
| p1 | 854 MiB | Control4 "Internal" data (ext4), untouched |
| p2 | 2 GiB | Control4 "Apps" (ext4, 15 MB used), shrunk from 6.7 GiB |
| p3 | 256 MiB | vfat `OHCBOOT`: `openhc.img` (U-Boot 2011.03 reads FAT, not ext4 extents) |
| p4 | the rest | ext4 `ohcdata` |

**With the flasher** (the bundle carries the stock-side kit: `ohc_kexec.ko`,
`ohc-kexec.sh`, `ohc-boot-env.sh`):

```sh
ohc-flash install <unit> --images openhc-hc250-<version>.zip   # --method sd, the default
ohc-flash restore <unit>                  # Control4's IO firmware + factory restore
ohc-flash uninstall <unit> --images ...   # stock from the NAND, openHC kept on the SD
ohc-flash boot <unit> --images ...        # back into openHC (after a fallback, too)
```

From stock it kexecs openHC into RAM to do the SD. The module loads into every
Control4 2.6.32 kernel, not just the one it was built against: it carries export
CRCs, so the loader skips vermagic's release, and the kernel functions it calls
it finds through kallsyms (proven on 2.10.6 and on the factory-restore image,
2.5.3). If a kexec ever fails, the env is written already and `install` boots the
openHC already on the SD instead. `install` from stock sets `ohc_arm=1`, which makes the
next `bootohc` zero the try count through U-Boot, so a stock that cannot touch the
EEPROM still points the box back at openHC. Proven on the unit 2026-11-09:
restore → install (from 2.5.3) → uninstall → boot, with no hands on the box.

**By hand**, the same steps:

```sh
# 1. on stock: kexec openHC (above)
# 2. on openHC: SD + EEPROM
ohc-sd-install install http://<host>/openhc-hc250-kernel.img
# 3. reboot to stock, then: the env
/tmp/ohc-boot-env.sh install        # board/hc250/stock-kexec/ohc-boot-env.sh
reboot                              # -> openHC from the SD
```

To leave without a factory restore: `ohc-sd-install stock` on openHC (every boot
goes to stock), then `ohc-boot-env.sh uninstall` on stock. Both scripts refuse
unless the SD layout, `bootcmd` and `restoresystem` are stock's (or already theirs).

The EEPROM shares I2C1 with the TPS65950. At the TWL's 2.6 MHz high-speed mode
it never answers, so the bus runs at 400 kHz.

## Audio and video (2026-11-08)

Both are described from what stock does on a live unit, and from Control4's
U-Boot and X-Loader GPL patches.

| Output | How | Status |
|---|---|---|
| Rear audio out / line in | TPS65950 codec on McBSP2 (`ti,omap-twl4030`); DAC2 into HSOL/HSOR like stock, line in on AUXL/AUXR | **proven**: 1 kHz out → cable → in, 100% tone at −30 dBFS, gone when muted, L→L with −52 dB crosstalk |
| Splash | omapdrm fbdev, 1280×720×32; fbcon and the Tux logo off | **proven** on an SD boot (`Splash: OK`) |
| HDMI | DSS DPI, 24 lines → ADV7513 (`adv7511`), 720p forced on (`video=HDMI-A-1:1280x720@60e`) | **proven**: splash on a 1080p monitor at 720p (pixel clock 72 MHz, the closest DPLL4 gets to 74.25); EDID read and HDMI mode when the display is on at power-on |
| HDMI audio | McBSP4 → ADV7513 I2S (`simple-audio-card`), 44.1 kHz only (patch 0001) | **proven**: a tone out of the monitor's speakers |
| Component | THS8200 from userspace (`S47video`, the HC-800's `ths8200` tool), registers captured off this unit at 720p60 | configured (137 registers, VERSION 0x04); no TV tried |

**HDMI audio clock.** The ADV7513 has no MCLK here and plays only with a 64 fs
bit clock (silent at 40 fs). McBSP4 is the bit-clock provider off the 96 MHz
PER clock, and with a generic card only a machine driver could set its divider,
so it stays at 1: a 96 MHz bit clock. Patch 0001 (`board/hc250/patches/linux`,
written to go upstream) gives omap-mcbsp `set_tdm_slot`. The DT asks for two
32-bit slots, and the driver derives the divider and offers only the rates it
lands within 0.1%. From 96 MHz that is
44.1 kHz (+400 ppm), the rate the HDMI dmix and every receiver already use; 48 kHz
would be 0.8% off. The connector is forced on so component keeps its DPI clock,
which also means a display plugged in after power-on gets no EDID read (DVI mode,
no audio) until the next boot.

**Receivers.** audiod runs one AirPlay 2 (shairport-sync, one shared nqptp) and
one Spotify Connect receiver per output: "HC-250 Analog" and "HC-250 HDMI", both
advertised over mDNS, idle load 0.65 with 335 MB free. The music library (mpd)
plays to either. AirPlay 2 from a phone to "HC-250 Analog" plays clean, with
shairport-sync and nqptp at 12–48% of the one core and the load around 1.

`S46audio` sets stock's codec path at boot (DAC2 → HSOL/HSOR, AUX → TX1). The codec
is the I2S clock master and DAPM keeps it powered down until a path reaches an
output: without it every PCM on the card stalls once McBSP2's FIFO (640 frames)
fills. audiod bounds its PCM-open probe so that cannot stall the daemon.

Details that came from the unit:

- **The display pads** use the DM3730 layout Beagle-xM does: dss_data0–5 on the
  data18–23 balls (mode 3), dss_data18–23 on sys_boot0/1/3–6. Stock's kernel sets
  this and U-Boot does not, so the DT carries the pinctrl. `vdds_dsi` is VPLL2.
- **THS8200:** reset is GPIO97 (X-Loader holds it low). Its 720p register set
  differs from the HC-800's in 24 registers: same chip, different input wiring.
- **VAUX3 and VDAC stay on.** Stock never registers them, so they keep the PMIC's
  power-up state; openHC was turning them off as unused.
- **Kexec'd from stock, the display does not start.** Stock clocks DISPC from the
  DSI PLL (`DSS_CONTROL` 0x1), and after the hand-off the clock mux stays there
  (`DSS_CLK_STATUS` 0x82 against 0x81 on an SD boot). DISPC never raises an
  interrupt. Switching the mux back from the kexec module before shutdown did not
  take. RAM test boots run without a picture; SD boots have one.

## Wi-Fi: a BCM4319

The module is fitted. Under stock, raising `wifi_reset` (GPIO176) and pulling the
card-detect loop low makes it enumerate: SDIO `02d0:4319`, a **BCM4319**.
GPIO101 (`wifi_carddetect`) is wired back to GPIO94, MMC2's card-detect input.
Stock 2.10.6 carries the driver and firmware in the `/mnt/jffs2` partition
(`dhd/`, installed by dpkg), not the rootfs, and never loads them:

| File | What |
|---|---|
| `dhd.ko` | Broadcom DHD 4.218.248.26, built for 2.6.32-51.48 |
| `rtecdc.bin` | `4319b0sdio/sdio-g-cdc-full11n-reclaim-roml-wme-idsup-40m`, 4.218.248.26 (2011-03-17), 231,766 bytes |
| `wl-nvram.txt` | `bcm94319lcsdn4l` board file, 26 MHz crystal, 2.4 GHz only |

The firmware is FullMAC over SDIO with the BCDC protocol, which `brcmfmac`
speaks (its 4329 firmware is the same generation). It is Broadcom's and not
redistributable, so openHC would take it off the unit's own stock partition,
not ship it.

Mainline `brcmfmac` has no 4319 entry, so MMC2 stays disabled. The 4319 is a
Sonics SB chip like the 4329, which `brcmfmac` handles with a hard-coded core
list. Under mainline the card does not initialize yet either: `MMCHS_CON`
differs from stock's (0x600 against 0x10e00: CTPL, CLKEXTFREE).

## What the first image is

- **Kernel:** mainline 7.1.8, `omap2plus_defconfig` + `linux/hc250.fragment`.
- **Device tree:** `linux/dts/ti/omap/c4-omap3-hc250.dts`, on `omap36xx.dtsi`. It sits
  under `ti/omap/` like the in-tree boards, because `omap36xx.dtsi` pulls in its clock
  files by bare name.
  Stock U-Boot passes ATAGS, so the DTB is appended to the zImage
  (`ARM_APPENDED_DTB` + `ARM_ATAG_DTB_COMPAT`).
- **Pin mux is U-Boot's**, except the display and McBSP4 pads, which stock's kernel
  sets (above). The stock mux table was saved off the unit.
- **MAC:** `smsc911x` reads its address only from DT properties, which this
  U-Boot can't fill. `S05ethaddr` sets it from `eth=` (the script passes U-Boot's
  `${ethaddr}`), falling back to the copy in the board EEPROM at offset 8.
- **IO:** the `iomcu` feature. The IO-MCU node describes block 1 — four jacks,
  blaster, receiver, one relay, one contact — so `gpio-ohc-iomcu` names the lines
  and lirc devices exactly as on the HC-800. `firmware-name` is set: the driver
  writes openHC's LM3S1162 image over Control4's 03.26.15 at the first boot
  (8,644 bytes, about 3.5 s), then applies the pin table. A factory restore's
  stock OS puts Control4's image back.
- **NAND:** described with every partition `read-only` (see the install section).
- **Held lines:** `zigbee_reset` and `zigrf_enable` high, the 12 V enable high,
  and the Wi-Fi radio held in reset, all as stock leaves them.

Not in the image yet: Zigbee (the feature brings Node, too big for a RAM image)
and Wi-Fi (no driver, above).

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

- **THS8200** (component out). No mainline driver; driven from userspace like
  the HC-800's.
- **Control4's NAND ECC layout** differs from mainline's (above). Only a layout
  patch would let Linux write the NAND; nothing needs that today.
- **Audio** needs no driver patch: the TPS65950 codec is `ti,omap-twl4030` and
  HDMI audio is McBSP4 into the ADV7513, both expressible in the DT. Control4's
  `magneto` machine driver is not needed.
- **Wi-Fi** needs BCM4319 support added to `brcmfmac`, plus firmware.

The openHC driver changes for this board are in our own `gpio-ohc-iomcu`: a
combo jack may now have a single serial-enable line (the HC-250's PB0/PB1), not
only the EA's pair, and a silent part gets one reset pulse before the driver
gives up (above).

## Next

1. A picture on a TV: HDMI with a monitor attached, and component.
2. Zigbee: start the EM357 app from its bootloader at boot, then a root filesystem
   on `ohcdata` (Zigbee/Node is too big for RAM).
3. The HC-250 in the flasher, calling the two install scripts.
4. Wi-Fi: BCM4319 in `brcmfmac`, plus firmware.
5. What the MADC inputs measure.
6. openHC's own LM3S1162 firmware on this board's pins.
