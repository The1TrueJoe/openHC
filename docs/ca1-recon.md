# CA-1 live recon

Everything below came off a running **CA-1 (board revision 4)** over SSH as root,
on 2026-08-12. Same vendor default password as every other Control4 controller.

The CA-1 is **not** a variant of anything already in this tree. The EA family is
Intel CE5310 with a companion TM4C IO-MCU behind CEFDK; the IOX is a TI DaVinci
DM355. The CA-1 is a **Freescale i.MX6 SoloLite running U-Boot out of SPI-NOR**,
with no IO-MCU at all. It is by a wide margin the friendliest Control4 target in
this repo: the SoC has first-class mainline Linux and U-Boot support, the
bootloader is stock U-Boot with an unlocked console and a writable environment,
and **secure boot is not enforced** (see below).

## Identity

```
/proc/c4board/name          ca1
/proc/c4board/revision      4
/proc/c4board/type          0   (binary 000)
hostname                    ca1-000FFF528265
uname                       Linux 4.1.52-2.180.29 #2.180.29-ca1.1 SMP PREEMPT
                            Tue May 19 20:37:16 MDT 2020 armv7l
toolchain (/proc/version)   gcc 4.9.3 (crosstool-NG crosstool-ng-1.22.0)
cmdline                     console=ttymxc0,115200 root=/dev/mmcblk1p2 rootwait rw
U-Boot                      2014.04-2.260.16 (May 19 2020 - 20:31:32), "//CA-1 U-BOOT//"
eth0 MAC                    00:0F:FF:52:82:65
wlan0 MAC                   00:0F:FF:52:76:93
```

Unlike the EA family, `/proc/c4board/type` is **0** here and the board is
identified by `name` rather than by a type strap. U-Boot carries the same two
values as environment variables (`board_id=0`, `board_rev=4`) and uses them to
pick a DTB filename — see [Boot chain](#boot-chain).

**Board codename: `emmet`.** The recovery partition holds `kernel-emmet.deb`.
(Control4 codenames run to Lego: `ninjago` and `garmadon` on the EAs, `hammer`
on the IOX, `emmet` here.)

## SoC / memory

```
Freescale i.MX6 SoloLite (i.MX6SL), silicon rev 1.3
1x ARM Cortex-A9 r2p10 (ARMv7-A), VFPv3 + NEON, 48.00 BogoMIPS
MemTotal 1028116 kB (1 GB), 320 MB reserved as CMA (linux,cma)
```

**Single core** — the only uniprocessor board in the matrix. `CONFIG_SMP` is on
in the vendor kernel but there is one CPU node in the DT.

The i.MX6SL is the low-power, E-ink-oriented member of the i.MX6 family: no
SATA, no PCIe, no 3D GPU (only a 2D GC320 at `2200000.gpu`) and an EPDC
(`20f4000.epdc`) that this product does not use.

### The device tree is a lightly-edited EVK tree

```
model      = "Freescale i.MX6SL CA-1 Board"
compatible = "fsl,imx6sl-evk", "fsl,imx6sl"
```

Control4 started from `imx6sl-evk.dts` and never changed the `compatible`. That
has a practical consequence: **several devices in the vendor DT are not on this
board.** The I²C bus enumerates

```
0-0008  pfuze100     PMIC            — real
0-0068  bq32000      RTC             — real
0-0010  elan-touch   touchscreen     — EVK leftover
0-001c  mma8450      accelerometer   — EVK leftover
0-0048  max17135     E-ink PMIC      — EVK leftover
```

Do not treat the vendor DT as a description of the hardware without checking
whether the driver actually bound. Two more leftovers that matter:

* `usdhc1`/`usdhc2` carry `cd-gpios`/`wp-gpios`. There is no card slot and no
  write-protect switch on either — usdhc1 is a soldered SDIO radio and usdhc2 is
  a soldered eMMC.
* The `bus-width` properties are **backwards relative to the pinmux**. The DT
  says usdhc1 is 8-bit and usdhc2 is 4-bit; the pin groups say usdhc1 has 6 pins
  (CLK/CMD/DAT0-3 = 4-bit) and usdhc2 has 10 (CLK/CMD/DAT0-7 = 8-bit). The pins
  are ground truth. usdhc1 = 4-bit SDIO Wi-Fi, usdhc2 = 8-bit eMMC.

## Boot chain

This is the important structural difference from every other board in the tree,
and it is all good news.

```
i.MX6SL boot ROM
  └─ SPI-NOR (ecspi2 CS0, Spansion S25FL128S, 16 MB)
       └─ IVT at flash offset 0x400 → U-Boot 2014.04 → DRAM 0x87800000
            └─ eMMC (usdhc2, 3.7 GB Micron M62704)
                 ├─ p1  254 MB  vfat  "kernel"  zImage + DTBs
                 ├─ p2  1.9 GB  ext4  "rootfs"  ← root=/dev/mmcblk1p2
                 └─ p3  1.4 GB  ext4  "recfs"   recovery payload
```

SPI-NOR layout (from `/proc/mtd`, matching `mtdparts` in the U-Boot env):

| mtd | offset | size | name |
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
   `fatload mmc 1:1 ${loadaddr} boot.scr` and `bootscript` is `source`. There is
   **no `boot.scr` on p1 today**, so dropping one there is a clean, reversible
   takeover of the boot path that needs no serial console, no soldering, and no
   bootloader reflash. Delete the file to go back to stock. This is the
   recommended bring-up loop for this board.
2. **`zImage` + DTB on the FAT partition** — replace or add alongside.
3. **TFTP** (`loadtftp` / `netboot`), with `image_path=ca1`, `image=zImage`.
   `netboot` even does the DHCP itself.

`set_fdt_file` selects the DTB by board ID and revision:

```
fdt_file         = c4-imx6sl-${board_id}-${board_rev}.dtb   → c4-imx6sl-0-4.dtb
fdt_default_file = c4-imx6sl.dtb
```

p1 ships `c4-imx6sl-0-3.dtb`, `c4-imx6sl-0-4.dtb` and `c4-imx6sl.dtb`; the
`-0-4` and the default are byte-identical in size and this rev-4 unit loads
`c4-imx6sl-0-4.dtb`.

Other useful env entries: `factoryrestore` boots the recovery kernel from
SPI-NOR; `flashspiuboot` reflashes U-Boot over TFTP
(`sf erase 0 f0000; sf write ${fileaddr} 400 efc00`, confirming the ROM reads
the IVT at flash offset 0x400); `mfgmode` boots a manufacturing image when DHCP
hands back `dhcp_mfgmode=3`; `dhcp_vendor-class-identifier=c4_ca1`.

`bootdelay=2` and `stdin=serial` — but **the serial console does NOT drop to a
U-Boot prompt on a keypress.** This was verified the hard way on hardware and
then confirmed from Control4's own U-Boot source (`OS-3.3.1`, board config
`include/configs/mx6slevk.h`, patch `260-uboot-enable-sha256-password-hash`):
the autoboot break-in is **SHA-256 password-gated**, exactly like the EA's
CEFDK. Control4's `CONFIG_KEYED_BOOTDELAY` build prints no "Hit any key" prompt,
reads a line, SHA-256's it, and compares against a baked-in 32-byte digest —
the *identical* hash as the EA (`ec 89 70 13 ad f9 …`), which is not in the GPL
drop. Any wrong key just lets it autoboot. **So there is no way into the U-Boot
console over serial.** Do not plan around one.

`fw_printenv`/`fw_setenv` are both present in the rootfs with a valid
`/etc/fw_env.config`, so **the environment is readable and writable from Linux**
without any console at all — which, given the locked U-Boot console, is the
*only* way to change the environment.

### Recovery when a bad boot.scr / kernel hangs the box (proven on hardware)

A broken `boot.scr` or a kernel that hangs after `Starting kernel ...` leaves
the box in a watchdog reboot loop with no network and no U-Boot console. The
way out, all pre-`bootcmd` and needing no password:

1. **Hold the recessed factory-restore button (gpio1,15 — NOT the main ID
   button, gpio1,17) and apply power, keep holding ~10 s.** U-Boot's
   `check_factoryrestore()` reads that pin in its init sequence, *before*
   `bootcmd` runs `boot.scr`, and boots the stock recovery kernel from SPI-NOR.
   LED goes yellow. (Confirms with `C4FR: Active` on the console.)
2. The recovery kernel's initramfs offers a **2-second `c4`+ENTER break-in to a
   root BusyBox shell** — and unlike U-Boot, this is NOT password-gated. Spam
   `c4\r\n` over serial (~every 0.4 s, low rate to avoid TX→RX crosstalk) while
   it boots to land in the window, *before* it reimages.
3. In that shell the eMMC is `/dev/mmcblk1`. Mount p1
   (`mount -t vfat /dev/mmcblk1p1 /mnt`), remove/rename the offending `boot.scr`,
   `sync`, `umount`, power-cycle. With no `boot.scr`, `bootcmd` falls through to
   the stock `zImage` and boots stock Control4.

Note: a **full** factory restore (letting the recovery kernel run, ~3 min,
yellow LED) reimages p2 (rootfs) and rewrites the stock kernel/DTBs on p1, but
does **not** delete extra files like our `boot.scr` — so it alone does not break
the hang loop. You must remove `boot.scr` via the `c4` initramfs shell.

Serial TX gotchas that cost real time: set `-hupcl clocal` (so closing the port
doesn't pulse DTR and reset the board), keep ONE persistent fd open rather than
reopening per keystroke, and keep the send rate low.

## Secure boot: HAB is open, and nothing else is verified either

The question was whether i.MX High Assurance Boot is enforced. It is not, on
three independent grounds.

**1. The `SEC_CONFIG` fuse reads open.** `/sys/fsl_otp/HW_OCOTP_CFG5` is
`0x00000000`. `SEC_CONFIG[1]` is bit 1 of that word; `0` is Open, `1` is Closed.
The same read shows `BT_FUSE_SEL` (bit 4) clear, meaning the boot fuses are not
committed and the ROM takes its boot device from the GPIO straps — which also
implies the USB serial-downloader (SDP) recovery path is still reachable.

> Caveat on the fuse read: the vendor `fsl_otp` driver returns
> `HW_OCOTP_CRC0 = 0xbadabada` and then reads empty for every fuse after it, and
> stays wedged until reboot — so `SRK0..7` could not be read this way. `CFG5`
> was read *before* the driver wedged, in the same pass as the plausible
> `CFG0=0xd5ad0523` / `CFG1=0x1b3d99d4` unique-ID words. Cross-checking against
> the OCOTP shadow registers at `0x021BC400` was not possible: `/dev/mem` exists
> and `CONFIG_STRICT_DEVMEM` is off, but `read()` on device memory returns
> `EFAULT` on ARM (it needs `mmap`), and the rootfs has no interpreter or
> `devmem` applet to do that. Hence indicator 2.

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

That slot lands at file offset `0x69000`, and it is **8 KB of zeros** — no
`0xD4` CSF tag, no certificate, no signature. The binary does contain the HAB
machinery (`hab_status`, `hab_auth_img`, `"Secure boot enabled"`,
`"Authenticate zImage Fail, Please check"`), i.e. it was compiled with
`CONFIG_SECURE_BOOT` and reserves `CONFIG_CSF_SIZE`, but the build never ran
`cst` over it. A Closed part would parse that zero-filled slot, fail
authentication and refuse to boot. **The unit boots, therefore the part is
Open** — which is what indicator 1 says independently.

**3. Even a closed ROM would not cover the kernel.** HAB only authenticates what
the ROM loads (U-Boot). Everything after that is up to the bootloader, and this
`bootcmd` never calls `hab_auth_img`: the kernel is loaded with a plain `bootz`
from a FAT partition, and the DTB likewise. There is no dm-verity, no signed
rootfs, no measured boot anywhere in the chain. So the `boot.scr` and
`zImage`-replacement paths above are unverified by construction, independent of
the fuse state.

**Definitive confirmation**, if wanted, is one command at the U-Boot prompt over
serial:

```
hab_status
```

On an Open part it prints `HAB Configuration: 0xf0, HAB State: 0x66` and
`No HAB Events Found!`. That is worth doing before anyone relies on this for
something irreversible, but the fuse and the empty CSF already agree.

## Peripherals

### UARTs — five, and the numbering is not the address order

The i.MX6SL puts UART5 at `0x02018000`, below UART1 at `0x02020000`, and the DT
aliases follow function rather than address. The resulting map:

| alias | node | i.MX UART | `/dev` | RTS/CTS | use |
|---|---|---|---|---|---|
| serial0 | `serial@02020000` | UART1 | `ttymxc0` | no | **console** @115200 |
| serial1 | `serial@02024000` | UART2 | `ttymxc1` | yes | unused / spare |
| serial2 | `serial@02034000` | UART3 | `ttymxc2` | yes | **RS-232/RS-485 port** (`ioserver`) |
| serial3 | `serial@02038000` | UART4 | `ttymxc3` | no | **Z-Wave** → `/dev/ttySZwave` |
| serial4 | `serial@02018000` | UART5 | `ttymxc4` | yes | **Zigbee** → `/dev/ttySZigbee` |

Verified by walking `/proc/*/fd`: `init` holds `ttymxc0`, `ioserver` holds
`ttymxc2`, `zwaved` holds `ttymxc3`.

### There is no IO-MCU

`ioserver` opens `/dev/ttymxc2` **directly**. On the EA family `ioserver` talks
to a TM4C1231D5 over `ttyS1` at 460800 and the MCU owns the IR, relays, contacts
and the combo serial ports; on the CA-1 there is no such part and no such
protocol. The rear serial port is a host UART wired to a transceiver that the
host reconfigures over GPIO.

`/control4/firmware/io/` still ships TM4C1231D5 and LM3S1162 images, but those
are for *external* accessories the controller flashes over its RS-485 gateway,
not for anything on this board.

### The RS-232/RS-485 combo port is GPIO-configured

Seven GPIOs configure the transceiver in front of UART3. This is the CA-1's
analogue of the EA's MCU-routed combo ports, but it is plain Linux serial plus
seven pins:

| `/dev/gpio` name | GPIO | boot dir/value |
|---|---|---|
| `serial_232_485` | 46 | out, 0 — protocol select |
| `serial_duplex` | 40 | out, 0 — half/full duplex |
| `serial_dx_en` | 41 | out, 1 — driver enable |
| `serial_rx_en` | 44 | out, 0 — receiver enable |
| `serial_te485` | 45 | out, 0 — RS-485 transmit enable |
| `serial_fen` | 50 | out, 0 — failsafe enable |
| `serial_loopback` | 51 | out, 0 — loopback test |

### Radios

**Z-Wave — Sigma Designs ZM5304** (500-series, ZW0500), on UART4. Identified
from `/control4/firmware/zwave/`, which contains only
`serialapi_controller_static_ZM5304_<region>.hex` (HK/KR/MY/IN/JP/RU/…).
`zwaved` is running and talking to it. `/dev/gpio/zwave_module` is a **presence
sense input** — it reads `1`, so the module is fitted. `zwave_reset` is GPIO 16.

The unit also supports an *external USB* Z-Wave dongle (FTDI `0403:6010`,
manufacturer `Control4`) via `/etc/udev/rules.d/80-zwave.rules`, symlinked
`ttyUSBZwave`; `/etc/zwaved/zwave.conf` sets `interfacePriority=localInterface`,
so the onboard module wins.

**Zigbee — EM35x NCP on UART5, and it did not answer.** `zigbee_reset` is
GPIO 8. The Zigbee stack is a library inside `director`
(`/control4/lib/libzigbeemanager.so`), not a daemon, and it is not running on
this unpaired unit — nothing holds `ttymxc4`. A direct EZSP/ASH probe
(`1A C0 38 BC 7E`) at 115200/57600/38400, with and without RTS/CTS, and again
after pulsing `zigbee_reset`, returned **zero bytes**; a bare CR/LF got no
bootloader prompt either. So the exact part is **not confirmed at runtime**.
Control4 uses the EM357 everywhere else and ships
`em357-uart-rts-cts-use-with-serial-uart-bootloader_4720.ebl`, and UART5 is the
one radio UART with RTS/CTS — all consistent with an EM357, but the NCP is
either unprogrammed or needs a bring-up step not yet found. Treat as EM357 `?`.

### Wi-Fi + Bluetooth — one Realtek RTL8723BS

SDIO device `024c:b723` on `mmc0` (usdhc1), driven by an out-of-tree
`8723bs.ko` (1.25 MB, the only module loaded). `wlan0` exists and reports
`<WIFI@REALTEK>`; it is unassociated on this unit. The same package is the
Bluetooth radio, which is why the GPIO table has `bt_enable` (18),
`bt_wake_host` (39, in) and `host_wake_bt` (42); `bluetoothd` is running.

**This is the one peripheral where mainline is clearly better placed than the
vendor:** `drivers/staging/rtl8723bs` has been in mainline since v4.12, so the
1.25 MB out-of-tree blob-adjacent module can be dropped for an in-tree driver.
The part needs a 32.768 kHz slow clock, which on this board is generated by
**PWM3** — the vendor DT node is `wireless-pwm-32K` with
`compatible = "pwm-control"`, an out-of-tree shim. Mainline wants a
`pwm-clock` / `clk-pwm` provider plus `mmc-pwrseq-simple` instead.

### Ethernet, USB, LEDs, button

* **Ethernet**: FEC at `2188000.ethernet`, `phy-mode = "rmii"`, PHY reset on
  GPIO 49 (`eth_phy_reset`). Single port.
* **USB**: two EHCI root hubs, both host, nothing attached during recon. Port
  power and over-current are handled by an out-of-tree `control4,usb-overcurrent`
  driver with two `usb_oc` entries, each holding a `gpio_pwr` and a `gpio_oc`.
  Mainline covers this with a `regulator-fixed` (`gpio` + `enable-active-high`)
  as `vbus-supply` plus `over-current-active-low` on the USB node.
* **LEDs**: an RGB status LED on three PWMs — `pwm1` red, `pwm2` blue, `pwm4`
  green, all via `pwm-leds`. (`pwm3` is the Wi-Fi 32 kHz clock, not an LED.)
* **Button**: one `gpio-keys` entry emitting `KEY_F5` (0x3f) — the front
  setup/identify button.

### Full GPIO map

From `/etc/init.d/c4gpio`, which is the entire board bring-up sequence — plain
sysfs exports plus symlinks into `/dev/gpio`. Linux numbering, so
bank = N/32 + 1, pin = N%32:

| GPIO | bank.pin | dir | name | value at recon |
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

## Userspace

Debian-ish (`dpkg`, `apt`, `/etc/init.d` + `/etc/rc.d`), busybox 1.27.1,
dropbear, syslog-ng. Control4 stack under `/control4`: `director` (the big one),
`c4server`, `ioserver`, `zwaved`, `sysmand`, `upmand`, `sddpd`, `imaged`,
`shaird`, `netusbserver`, `led_service`, `c4faultd`, plus a Node broker
(`/mnt/internal/node/broker/broker.js`), nginx, Samba and an atftpd serving
`/control4/firmware/`.

Nothing here is needed by openHC; it is listed so the recovery image can be told
apart from ours, and because `ioserver`/`zwaved` are the two processes whose
behaviour we eventually have to reproduce.

## What this means for the port

The CA-1 is the first board in this tree where **mainline already supports the
SoC**. No resurrection patches (contrast the DM355 in
[kernel-7.1-port.md](kernel-7.1-port.md)), no CEFDK container wrapping (contrast
the EAs). `imx_v6_v7_defconfig` covers i.MX6SL; what the board needs is a DTS and
a rootfs.

Work that remains, roughly in order:

1. **A mainline DTS.** First draft is at
   `board/ca1/linux/c4-imx6sl-ca1.dts`, built from the pin groups decoded out of
   the vendor DTB. Its `fsl,pins` entries are raw 6-tuples rather than
   `MX6SL_PAD_*` macros — correct, but a readability cleanup is owed.
2. **Boot it via `boot.scr`.** Non-destructive, reversible, no serial needed.
3. **Wi-Fi on the in-tree `rtl8723bs`**, replacing the vendor `8723bs.ko`, which
   needs the PWM3 32 kHz clock re-expressed as a mainline clock provider.
4. **Zigbee NCP** — find out why the radio is silent before assuming EM357.
5. **RS-485 direction control.** The seven transceiver GPIOs have no mainline
   consumer; `rs485-rts-*` on the UART node plus a small userspace helper is the
   likely shape.

## Open questions

* The exact Zigbee part, and why the NCP does not respond (see above).
* `SRK0..7` fuses were never read — irrelevant while `SEC_CONFIG` is open, but
  it would confirm whether Control4 ever burned a key hash. Needs an `mmap`
  helper or `hab_status` from U-Boot.
* Whether `board_id` is ever non-zero (a CA-1 variant, or the CA-10 sharing the
  DTB naming scheme).
* Which of `pwm1`/`pwm2`/`pwm4` drives which physical LED colour was taken from
  the DT `label`s, not measured.
* `mtd4` "Reserved" (2.9 MB) — contents unexamined.
* Whether the `Recovery Kernel` in SPI-NOR (`factoryrestore`) is a full recovery
  ramdisk or just a kernel that then mounts p3.
