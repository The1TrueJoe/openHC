---
title: EA-3 recon
description: A live pull off a running EA-3 rev 9 — and why it is much closer to an EA-1 than the matrix used to assume.
sidebar:
  order: 3
---

Everything below came off a running **EA-3 (board revision 9)** over SSH as root.

Read this alongside the [EA-1 recon](/ea/ea1/). **Same SoC, same board
codename, same eMMC layout, same bootloader, same IO-MCU firmware image.** The
interesting differences are the managed Ethernet switch, the third combo serial
port, the extra IR jacks, and the analog/coax audio path.

## Identity

```
/proc/c4board/name        ea3
/proc/c4board/revision    9
/proc/c4board/type        2   (binary 010)      EA1 is type 1
hostname                  ea3-000FFF94EE02
uname                     Linux 3.12.74 #8-140-ninjago.1 SMP PREEMPT i686
                          (was 3.12.17 #118, Aug 2018, before the factory restore)
```

## The factory restore, and why it was worth doing

This unit arrived on a 2018 build, kernel **3.12.17 #118**. It has since been
factory-restored from p2 and runs **3.12.74 #8-140-ninjago.1**, the same kernel
the EA1 runs, and the same version as the last GPL drop covering these boards.

That alignment is the point. `MODVERSIONS` is off, so an out-of-tree module's
`vermagic` string is the only compatibility gate, and `MODULE_FORCE_LOAD` is off
too, so it can't be overridden:

```
running vermagic   3.12.74 SMP preempt mod_unload ATOM
GPL drop           linux-3.12.74 + 54 Intel + 25 Control4 patches
EA1                3.12.74 #8-140-ninjago.1   — identical
```

**The restore itself is now proven on an EA3**, which it was not before. It ran
from the recovery button, extracted `recfs.tar.xz` over p1, and rebooted into a
working stock image in about a minute. There is a benign `device_shutdown`
warning in the reboot path. Everything checked afterwards was unchanged: switch
link summary still `0x24`, same IO-MCU images, three user-serial sockets, DSP
firmware present.

## SoC and memory — identical to the EA1

```
Intel Atom CE5310 @ 1.20GHz — 2 cores / 4 threads, family 6 model 54, stepping 2
MemTotal 1597992 kB (~1.5 GB), zram0 swap (2 GB backing)
```

The matrix's old `i686 ?` for EA3 is now confirmed rather than inferred.

Notable PCI functions, all on the SoC:

```
01:02.0 8086:089b  display   -> PowerVR SGX (pvrsrvkm)
01:16.0 8086:070a  display   -> Vivante GC300 2D (galcore)
01:0b.4 8086:2e6a  SPI master (pxa2xx-spi.0)  -> switch + spidev
01:0c.0 8086:2e6e  Ethernet  (e1000)
01:17.0 8086:08a0  SPI       -> SPI-NOR (nmyx25, mtd0)
01:1b.0 8086:070b  SD host   -> eMMC
```

## Storage

Same layout as the EA1. `p2` is the factory-restore payload, byte-for-byte the
same shape:

```
cefdk.deb            bootloader package        version 36-34
recovery_kernel.deb  boot kernel               3.12.74-8-140
recfs.tar.xz         factory rootfs (490 MB)   3.3.0.628678-res
common.hcfg
manifest             version strings + md5s
```

**No GRUB anywhere** — CEFDK loads the kernel directly, same as the EA1.

## Serial map

Identical to the EA1, including the wrong comment in `/etc/rc.d/99control4` that
calls ttyS2 an "8051 Power Management Inside CE53xx", it's a
[PIC24](/ea/hdmi-cec/).

**The three user-facing RS-232 ports are MCU-routed, not host ttys**, and
`ioserver` proves the count by the sockets it opens:

```
5100  dt_listen_port
5101  user serial port 1   \
5102  user serial port 2    >  three combo IR/serial jacks
5103  user serial port 3   /
20000 io_listen_socket
```

An EA1 opens 5101/5102 only. **This is the cleanest non-invasive way to count
user serial ports on any of these boards.**

## GPIO aliases

The EA3 carries everything the EA1 has plus a power and switch group:

```
shared with EA1   io_reset(7) zigbee_reset(29) codec_reset(24) dsp_reset(101)
                  wlan_disable(27) usb_2_serial_reset(26) board_id0..6(17..23)
EA3 additions     gb_eth_reset(5)      gigabit PHY reset
                  gb_sw_reset(8)       BCM53125 switch reset
                  poe_type(54)         PoE class strap
                  ldo_enable(43)  ac_pwr(80)
                  twelve_volt_ok(122)  n_twelve_volt_current_limit(75)
                  n_usb_current_limit(121)
                  n_fpga_reload(57)    present but unused
```

### The LED map, recovered from the stock kernel binary

The LEDs are kernel LED-class devices, not raw GPIO aliases, and the map is
**not** in the GPL drop, it lived in Control4's absent `ninjago_platform`.

It was recovered from the stock kernel image instead, which mattered because the
neighbouring lines are resets for the NIC, the switch and the codec, and probing
by hand would have been a bad idea. In the decompressed `vmlinux` the six LED
name strings are contiguous, the pointers to them form a 16-byte-stride
`struct gpio_led[]`, and the `gpio_led_platform_data` in front of it reads
`num_leds = 6`:

| LED | GPIO | Polarity |
|---|---|---|
| `c4::network` | 15 | active high |
| `warn::red` | 99 | active high |
| `warn::yellow` | 16 | active high |
| `warn::blue` | 100 | active high |
| `c4::4ball_red` | 102 | **active low** |
| `c4::4ball_blue` | 34 | **active low** |

Three of those — 99, 100 and 102 — are **above the 12 lines mainline's
`gpio-sodaville` exposes** for this controller. That's exactly why the front
panel was unreachable on a mainline kernel, and why openHC replaces that driver
with `gpio-intelce`, which exposes 128.

The same technique confirmed the audio codec's `i2c_board_info`: **adau1451 at
I²C address 0x38**, matching the live-box reading, a useful cross-check that the
extraction is reading real platform data rather than coincidence.

## Radios

```
Zigbee   CP2104 USB->UART (10c4:ea60) -> EM357-class NCP
Z-Wave   ZM5304 module, zwaved, region firmware in /control4/firmware/zwave/
Wi-Fi    NONE. No ath9k, no /sys/class/ieee80211, no wlan interface.
         The wlan_disable GPIO exists but is shared board support, not a radio.
```

The NCP part number can't be read from the host — USB shows only the CP2104
bridge — so "EM357-class" comes from the peripheral images Control4 ships, not
from a read of the die.

## I²C and SPI device map

```
i2c-0  (pxa_i2c)   0x48  lm75    temperature sensor
                   0x68  ds1339  RTC (rtc-ds1307)
i2c-1  (pxa_i2c)   -
i2c-2  (pxa_i2c)   -
i2c-3  (pxa_i2c)   0x38  adau1451  SigmaDSP

spi0.1  spi-bcm53125   managed switch
spi0.2  spidev         exported to userspace
spi0.3  spidev         exported to userspace
spi1.0  nmyx25         SPI NOR -> mtd0
```

That fourth I²C bus is the one mainline never creates. See
[audio](/ea/audio/).

### The AK4621 is not software-controlled at all

The **AK4621EF has no control path on this system**, and that is a positive
finding rather than a gap. Four independent checks:

```
grep -c ak4621 /proc/kallsyms          -> 0     no symbol anywhere in the kernel
grep ak46 /proc/modules                -> none  no module
readlink /proc/*/fd/* | grep spidev    -> none  nothing has spidev0.2 or 0.3 open
strings /control4/bin/* | grep spidev  -> none  no vendor binary mentions spidev
```

`ea3_dsploader` references exactly one device, `/dev/i2c-3`, the ADAU1451. The
only thing anyone does to the codec is release its reset at boot.

So the part is **strapped into standalone/hardware mode**: reset is deasserted,
I²S1 feeds it, and nothing ever configures it over a control port. For an open
image that's good news: the codec needs **no driver**, only `codec_reset` high
and I²S1 running.

## There is no FPGA on this board

The PCB has no FPGA and the software agrees, but only if you look past the
driver list, which is misleading:

```
loaded modules   snd_ninjago_fpga, snd_ninjago_fpga_pcm,
                 snd_soc_ninjago_fpga_dai, snd_soc_ninjago_fpga_dsp
registered drvs  pci:ninjago-fpga, platform:ninjago-fpga-dai, ...
bound devices    NONE — every driver directory contains only
                 bind/unbind/uevent, with no device symlinks
manifest         names garmadon-fpga-45t-spi-rev8.bin
```

The "ninjago" kernel is one build shared across the EA family, the FPGA audio
drivers ship unconditionally, and on an EA3 nothing probes. The image is
`garmadon-*`, and **garmadon is the EA5's codename**, which is where that
hardware lives. On an EA3 the ADAU1451 does the job the FPGA does on an EA5.

**`snd_ninjago_fpga*` in an `lsmod` isn't evidence of an FPGA.** Check
`/sys/bus/*/drivers/ninjago-fpga*/` for bound devices instead.

## What is not yet verified

- **Which physical rear jack maps to which IR channel index.**
- **The BCM53125 under mainline `b53`/DSA** — `e1000` alone does get a link now.
- **The Zigbee NCP part number** (EM357 vs the EM537 marking on the PCB).
- **The board-ID strap voltage.** The MCU's selector arithmetic is fully decoded
  and *predicts* ~0.60 V on PB4 for an EA3 and ~0.40 V for an EA1, but no one
  has measured it.
- **Which of the MCU's two four-pin groups is relays and which is contacts.**

### Closed since the first pass

- the AK4621 control path: there is none, the part is strapped standalone;
- the switch's CPU port (**5**, not the IMP at 8), the primary jack (**2**) and
  the second jack (**1**, isolated by the stock configuration);
- the MCU board-ID selector arithmetic;
- the contact's wire semantics — a u32 bitmask, EA3's contact is bit 0, closed
  reads 1, and the host must poll.
