---
title: CA-1
description: A live pull off a CA-1 rev 4 — i.MX6 SoloLite, stock U-Boot, open HAB, and the boot.scr way in.
sidebar:
  order: 1
  label: Overview & recon
---

Everything below came off a running **CA-1 (board revision 4)** over SSH as root.

The CA-1 is **not** a variant of anything else in this tree. It is a **Freescale
i.MX6 SoloLite running U-Boot out of SPI-NOR**, with no IO-MCU at all. By a wide
margin it is the friendliest Control4 target here: the SoC has first-class
mainline Linux and U-Boot support, the bootloader is stock U-Boot with a writable
environment, and **secure boot isn't enforced**.

## Identity

```
/proc/c4board/name          ca1
/proc/c4board/revision      4
/proc/c4board/type          0   (binary 000)
hostname                    ca1-000FFF528265
uname                       Linux 4.1.52-2.180.29 #2.180.29-ca1.1 SMP PREEMPT armv7l
U-Boot                      2014.04-2.260.16 (May 19 2020), "//CA-1 U-BOOT//"
eth0 MAC                    00:0F:FF:52:82:65
```

Unlike the EA family, `/proc/c4board/type` is **0** here and the board is
identified by `name`. U-Boot carries the same two values as environment variables
(`board_id=0`, `board_rev=4`) and uses them to pick a DTB filename.

**Board codename: `emmet`.** The recovery partition holds `kernel-emmet.deb`.

## SoC and memory

```
Freescale i.MX6 SoloLite (i.MX6SL), silicon rev 1.3
1x ARM Cortex-A9 r2p10 (ARMv7-A), VFPv3 + NEON
MemTotal 1028116 kB (1 GB), 320 MB reserved as CMA
```

**Single core**, the only uniprocessor board in the matrix. `CONFIG_SMP` is on in
the vendor kernel but there's one CPU node in the DT.

The i.MX6SL is the low-power, E-ink-oriented member of the family: no SATA, no
PCIe, no 3D GPU (only a 2D GC320) and an EPDC this product does not use.

## The device tree is a lightly-edited EVK tree

```
model      = "Freescale i.MX6SL CA-1 Board"
compatible = "fsl,imx6sl-evk", "fsl,imx6sl"
```

Control4 started from `imx6sl-evk.dts` and never changed the `compatible`. That
has a practical consequence: **several devices in the vendor DT are not on this
board.**

```
0-0008  pfuze100     PMIC            — real
0-0068  bq32000      RTC             — real
0-0010  elan-touch   touchscreen     — EVK leftover
0-001c  mma8450      accelerometer   — EVK leftover
0-0048  max17135     E-ink PMIC      — EVK leftover
```

Do not treat the vendor DT as a description of the hardware without checking
whether the driver actually bound. Two more leftovers that matter:

- `usdhc1`/`usdhc2` carry `cd-gpios`/`wp-gpios`. There's no card slot and no
  write-protect switch on either — usdhc1 is a soldered SDIO radio and usdhc2 is a
  soldered eMMC.
- **The `bus-width` properties are backwards relative to the pinmux.** The DT says
  usdhc1 is 8-bit and usdhc2 is 4-bit; the pin groups say usdhc1 has 6 pins
  (4-bit) and usdhc2 has 10 (8-bit). **The pins are ground truth.**

## Boot chain

This is the important structural difference from every other board here, and it
is all good news.

```
i.MX6SL boot ROM
  └─ SPI-NOR (ecspi2 CS0, Spansion S25FL128S, 16 MB)
       └─ IVT at flash offset 0x400 → U-Boot 2014.04 → DRAM 0x87800000
            └─ eMMC (usdhc2, 3.7 GB Micron M62704)
                 ├─ p1  254 MB  vfat  "kernel"  zImage + DTBs
                 ├─ p2  1.9 GB  ext4  "rootfs"  ← root=/dev/mmcblk1p2
                 └─ p3  1.4 GB  ext4  "recfs"   recovery payload
```

SPI-NOR layout, from `/proc/mtd`:

| mtd | Offset | Size | Name |
|---|---|---|---|
| mtd0 | 0x000000 | 16 MB | `SPI-NOR` (whole-chip) |
| mtd1 | 0x000000 | 960 KB | `U-Boot Bootloader` |
| mtd2 | 0x0f0000 | 64 KB | `U-Boot Environment` |
| mtd3 | 0x100000 | 12 MB | `Recovery Kernel` |
| mtd4 | 0xd00000 | 2.9 MB | `Reserved` |
| mtd5 | 0xff0000 | 64 KB | `U-Boot Redundant Environment` |

### `bootcmd` hands us three separate ways in

```
bootcmd = setenv hostname ca1-${ethaddr}; mmc dev ${mmcdev}; mmc dev ${mmcdev};
          run set_fdt_file;
          if mmc rescan; then
            if run loadbootscript; then run bootscript;      # ① boot.scr on p1
            else if run loadimage; then run mmcboot;         # ② zImage on p1
                 else run loadtftp; fi;                      # ③ TFTP
            fi;
          else run netboot; fi
```

1. **`boot.scr` on the FAT partition.** `loadbootscript` is
   `fatload mmc 1:1 ${loadaddr} boot.scr` and `bootscript` is `source`. **There is
   no `boot.scr` on the unit today**, so dropping one there's a clean, reversible
   takeover of the boot path that needs no serial console, no soldering and no
   bootloader reflash. Delete the file to go back to stock. **This is the
   recommended bring-up loop for this board.**
2. **`zImage` + DTB on the FAT partition** — replace or add alongside.
3. **TFTP** (`loadtftp` / `netboot`), with `image_path=ca1`, `image=zImage`.
   `netboot` even does the DHCP itself.

`set_fdt_file` selects the DTB by board ID and revision:

```
fdt_file         = c4-imx6sl-${board_id}-${board_rev}.dtb   → c4-imx6sl-0-4.dtb
fdt_default_file = c4-imx6sl.dtb
```

Other useful environment entries: `factoryrestore` boots the recovery kernel from
SPI-NOR; `flashspiuboot` reflashes U-Boot over TFTP (and its
`sf write ${fileaddr} 400 efc00` confirms the ROM reads the IVT at flash offset
0x400); `mfgmode` boots a manufacturing image when DHCP hands back
`dhcp_mfgmode=3`.

:::caution[There is no way into the U-Boot console over serial]
`bootdelay=2` and `stdin=serial`, but the console does **not** drop to a prompt on
a keypress. Verified the hard way on hardware and then confirmed from Control4's
own U-Boot source (patch `260-uboot-enable-sha256-password-hash`): the autoboot
break-in is **SHA-256 password-gated**, exactly like the EA's CEFDK. The build
prints no "Hit any key" prompt, reads a line, hashes it, and compares against a
baked-in 32-byte digest — **the identical hash as the EA**, which is not in the
GPL drop. Any wrong key just lets it autoboot. Do not plan around a U-Boot prompt.
:::

`fw_printenv`/`fw_setenv` are both present in the rootfs with a valid
`/etc/fw_env.config`, so **the environment is readable and writable from Linux** —
which, given the locked console, is the *only* way to change it.

### Recovery when a bad boot.scr hangs the box

Proven on hardware. A broken `boot.scr` or a kernel that hangs after
`Starting kernel ...` leaves the box in a watchdog reboot loop with no network and
no U-Boot console. The way out is all pre-`bootcmd` and needs no password:

1. **Hold the recessed factory-restore button (gpio1,15 — NOT the main ID button,
   gpio1,17) and apply power, keep holding ~10 s.** U-Boot's
   `check_factoryrestore()` reads that pin in its init sequence, *before* `bootcmd`
   runs `boot.scr`, and boots the stock recovery kernel from SPI-NOR. The LED goes
   yellow and the console confirms with `C4FR: Active`.
2. The recovery kernel's initramfs offers a **2-second `c4`+ENTER break-in to a
   root BusyBox shell**, and unlike U-Boot, **this is not password-gated.** Spam
   `c4\r\n` over serial (about every 0.4 s, low rate to avoid TX→RX crosstalk)
   while it boots, to land in the window *before* it reimages.
3. In that shell the eMMC is `/dev/mmcblk1`. Mount p1, remove or rename the
   offending `boot.scr`, `sync`, `umount`, power-cycle.

**A full factory restore does not fix this on its own.** Letting the recovery
kernel run (~3 min, yellow LED) reimages p2 and rewrites the stock kernel and DTBs
on p1, but does **not** delete extra files like our `boot.scr`, so the hang loop
survives it. You must remove the file via the `c4` shell.

Serial TX gotchas that cost real time: set `-hupcl clocal` so closing the port does
not pulse DTR and reset the board, keep **one** persistent fd open rather than
reopening per keystroke, and keep the send rate low.

## Secure boot: HAB is open, and nothing else is verified either

Not enforced, on three independent grounds.

**1. The `SEC_CONFIG` fuse reads open.** `/sys/fsl_otp/HW_OCOTP_CFG5` is
`0x00000000`. `SEC_CONFIG[1]` is bit 1; `0` is Open. The same read shows
`BT_FUSE_SEL` clear, meaning the boot fuses aren't committed and the ROM takes
its boot device from the GPIO straps, which also implies the USB serial-downloader
recovery path is still reachable.

:::note[Caveat on the fuse read]
The vendor `fsl_otp` driver returns `HW_OCOTP_CRC0 = 0xbadabada` and then reads
empty for **every fuse after it**, staying wedged until reboot, so `SRK0..7` could
not be read this way. `CFG5` was read *before* the driver wedged, in the same pass
as the plausible unique-ID words.

Cross-checking against the OCOTP shadow registers wasn't possible either:
`/dev/mem` exists and `CONFIG_STRICT_DEVMEM` is off, but **`read()` on device
memory returns `EFAULT` on ARM** (it needs `mmap`), and the rootfs has no
interpreter and no `devmem` applet. Hence indicator 2.
:::

**2. U-Boot is built for signing but was never signed.** The IVT at flash offset
0x400 parses cleanly and its CSF pointer is *populated*:

```
header    = 0x402000d1   (tag 0xd1, len 32, version 0x40)
entry     = 0x87800000
dcd       = 0x877ff42c
boot_data = 0x877ff420   → start 0x877ff000, length 0x6b000, plugin 0
self      = 0x877ff400
csf       = 0x87868000   ← non-zero: a signature slot exists
```

That slot lands at file offset `0x69000`, and it is **8 KB of zeros**, no `0xD4`
CSF tag, no certificate, no signature. The binary *does* contain the HAB machinery
(`hab_status`, `hab_auth_img`, `"Secure boot enabled"`), so it was compiled with
`CONFIG_SECURE_BOOT` and reserves the space; the build just never ran the signing
tool. **A Closed part would parse that zero-filled slot, fail authentication and
refuse to boot. The unit boots, therefore the part is Open**, which is what
indicator 1 says independently.

**3. Even a closed ROM would not cover the kernel.** HAB only authenticates what
the ROM loads. This `bootcmd` never calls `hab_auth_img`: the kernel is loaded
with a plain `bootz` from a FAT partition, and the DTB likewise. There's no
dm-verity, no signed rootfs, no measured boot anywhere in the chain. So the
`boot.scr` and `zImage`-replacement paths are unverified **by construction**,
independent of the fuse state.

**Definitive confirmation**, if wanted, is one command at a U-Boot prompt:
`hab_status`. On an Open part it prints `HAB Configuration: 0xf0, HAB State: 0x66`
and `No HAB Events Found!`. Worth doing before anyone relies on this for something
irreversible, though the fuse and the empty CSF already agree.

## Peripherals

### UARTs — five, and the numbering is not the address order

The i.MX6SL puts UART5 at `0x02018000`, *below* UART1 at `0x02020000`, and the DT
aliases follow function rather than address:

| Alias | i.MX UART | `/dev` | RTS/CTS | Use |
|---|---|---|---|---|
| serial0 | UART1 | `ttymxc0` | no | **console** @115200 |
| serial1 | UART2 | `ttymxc1` | yes | unused / spare |
| serial2 | UART3 | `ttymxc2` | yes | **RS-232/RS-485 port** (`ioserver`) |
| serial3 | UART4 | `ttymxc3` | no | **Z-Wave** → `/dev/ttySZwave` |
| serial4 | UART5 | `ttymxc4` | yes | **Zigbee** → `/dev/ttySZigbee` |

Verified by walking `/proc/*/fd`: `init` holds `ttymxc0`, `ioserver` holds
`ttymxc2`, `zwaved` holds `ttymxc3`.

### There is no IO-MCU

`ioserver` opens `/dev/ttymxc2` **directly**. On the EA family it talks to a
TM4C1231D5 over a UART and the MCU owns the IR, relays, contacts and combo serial
ports; on the CA-1 there's no such part and no such protocol. The rear serial
port is a host UART wired to a transceiver that the host reconfigures over GPIO.

`/control4/firmware/io/` still ships TM4C1231D5 and LM3S1162 images, but those are
for *external* accessories the controller flashes over its RS-485 gateway, not for
anything on this board.

### The RS-232/RS-485 combo port is GPIO-configured

Seven GPIOs configure the transceiver in front of UART3, the CA-1's analogue of
the EA's MCU-routed combo ports, but plain Linux serial plus seven pins:

| `/dev/gpio` name | GPIO | Boot dir/value |
|---|---|---|
| `serial_232_485` | 46 | out, 0 — protocol select |
| `serial_duplex` | 40 | out, 0 — half/full duplex |
| `serial_dx_en` | 41 | out, 1 — driver enable |
| `serial_rx_en` | 44 | out, 0 — receiver enable |
| `serial_te485` | 45 | out, 0 — RS-485 transmit enable |
| `serial_fen` | 50 | out, 0 — failsafe enable |
| `serial_loopback` | 51 | out, 0 — loopback test |

### Radios

**Z-Wave — Sigma Designs ZM5304** (500-series), on UART4. Identified from
`/control4/firmware/zwave/`, which contains only
`serialapi_controller_static_ZM5304_<region>.hex`. `zwaved` is running and talking
to it. `/dev/gpio/zwave_module` is a **presence sense input**. It reads `1`, so
the module is fitted.

The unit also supports an *external USB* Z-Wave dongle via a udev rule, but
`interfacePriority=localInterface` means the onboard module wins.

**Zigbee — EM35x NCP on UART5, and it did not answer.** The Zigbee stack is a
library inside `director`, not a daemon, and it isn't running on this unpaired
unit; nothing holds `ttymxc4`. A direct EZSP/ASH probe (`1A C0 38 BC 7E`) at
115200/57600/38400, with and without RTS/CTS, and again after pulsing
`zigbee_reset`, returned **zero bytes**; a bare CR/LF got no bootloader prompt
either.

So the exact part is **not confirmed at runtime**. Control4 uses the EM357
everywhere else, ships the matching `.ebl`, and UART5 is the one radio UART with
RTS/CTS — all consistent with an EM357, but the NCP is either unprogrammed or
needs a bring-up step not yet found. Treat as EM357 `?`.

### Wi-Fi + Bluetooth — one Realtek RTL8723BS

SDIO device `024c:b723` on usdhc1, driven by an out-of-tree `8723bs.ko`, the only
module loaded. The same package is the Bluetooth radio, which is why the GPIO
table has `bt_enable`, `bt_wake_host` and `host_wake_bt`.

**This is the one peripheral where mainline is clearly better placed than the
vendor:** `drivers/staging/rtl8723bs` has been in mainline since v4.12, so the
1.25 MB out-of-tree module can be dropped for an in-tree driver.

The part needs a 32.768 kHz slow clock, which on this board is generated by
**PWM3** — the vendor DT node is `wireless-pwm-32K` with an out-of-tree
`compatible`. Mainline wants a `pwm-clock` provider plus `mmc-pwrseq-simple`
instead.

### Ethernet, USB, LEDs, button

- **Ethernet:** FEC at `2188000.ethernet`, `phy-mode = "rmii"`, PHY reset on GPIO
  49. Single port.
- **USB:** two EHCI root hubs, both host. Port power and over-current are handled
  by an out-of-tree `control4,usb-overcurrent` driver. Mainline covers this with a
  `regulator-fixed` as `vbus-supply` plus `over-current-active-low` on the USB node.
- **LEDs:** an RGB status LED on three PWMs — `pwm1` red, `pwm2` blue, `pwm4`
  green, via `pwm-leds`. (`pwm3` is the Wi-Fi 32 kHz clock, not an LED.)
- **Button:** one `gpio-keys` entry emitting `KEY_F5` — the front setup/identify
  button.

### Full GPIO map

From `/etc/init.d/c4gpio`, which is the entire board bring-up sequence. Linux
numbering, so bank = N/32 + 1, pin = N%32:

| GPIO | bank.pin | Dir | Name | Value |
|---|---|---|---|---|
| 8 | 1.8 | out high | `zigbee_reset` | 1 |
| 16 | 1.16 | out high | `zwave_reset` | 1 |
| 18 | 1.18 | out low | `bt_enable` | 0 |
| 39 | 2.7 | in | `bt_wake_host` | 1 |
| 40 | 2.8 | out low | `serial_duplex` | 0 |
| 41 | 2.9 | out high | `serial_dx_en` | 1 |
| 42 | 2.10 | out high | `host_wake_bt` | 1 |
| 44 | 2.12 | out low | `serial_rx_en` | 0 |
| 45 | 2.13 | out low | `serial_te485` | 0 |
| 46 | 2.14 | out low | `serial_232_485` | 0 |
| 49 | 2.17 | out high | `eth_phy_reset` | 1 |
| 50 | 2.18 | out low | `serial_fen` | 0 |
| 51 | 2.19 | out low | `serial_loopback` | 0 |
| 92 | 3.28 | in | `zwave_module` | 1 (module present) |
| 94 | 3.30 | out high | `wifi_enable` | 1 |

## What this means for the port

The CA-1 is the first board in this tree where **mainline already supports the
SoC**. No resurrection patches, no bootloader container wrapping.
`imx_v6_v7_defconfig` covers i.MX6SL; what the board needs is a DTS and a rootfs.

Work remaining, roughly in order:

1. **A mainline DTS.** First draft is at `board/ca1/linux/c4-imx6sl-ca1.dts`, built
   from the pin groups decoded out of the vendor DTB. Its `fsl,pins` entries are
   raw 6-tuples rather than `MX6SL_PAD_*` macros — correct, but a readability
   cleanup is owed.
2. **Boot it via `boot.scr`.** Non-destructive, reversible, no serial needed.
3. **Wi-Fi on the in-tree `rtl8723bs`**, which needs the PWM3 32 kHz clock
   re-expressed as a mainline clock provider.
4. **Zigbee NCP**, find out why the radio is silent before assuming EM357.
5. **RS-485 direction control.** The seven transceiver GPIOs have no mainline
   consumer; `rs485-rts-*` on the UART node plus a small userspace helper is the
   likely shape.

## Open questions

- The exact Zigbee part, and why the NCP does not respond.
- `SRK0..7` fuses were never read, irrelevant while `SEC_CONFIG` is open, but it
  would confirm whether Control4 ever burned a key hash.
- Whether `board_id` is ever non-zero (a CA-1 variant, or a CA-10 sharing the DTB
  naming scheme).
- Which of `pwm1`/`pwm2`/`pwm4` drives which physical LED colour was taken from the
  DT labels, not measured.
- `mtd4` "Reserved" (2.9 MB) — contents unexamined.
- Whether the SPI-NOR `Recovery Kernel` is a full recovery ramdisk or just a
  kernel that then mounts p3.
