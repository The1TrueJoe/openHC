# EA3 live recon

Everything below came off a running **EA-3 (board revision 9)** over SSH as root,
on 2026-08-12. Same vendor default password as every other Control4 controller.

Read this alongside [ea1-recon.md](ea1-recon.md) — the EA3 is much closer to the
EA1 than the hardware matrix used to assume. **Same SoC, same board codename,
same eMMC layout, same bootloader, same IO-MCU firmware image.** The interesting
differences are the managed Ethernet switch, the third combo serial port, the
extra IR jacks, and the analog/coax audio path.

## Identity

```
/proc/c4board/name        ea3
/proc/c4board/revision    9
/proc/c4board/type        2   (binary 010)      EA1 is type 1
hostname                  ea3-000FFF94EE02      (MAC 00:0F:FF:94:EE:02)
uname                     Linux 3.12.74 #8-140-ninjago.1 SMP PREEMPT
                          Wed May 25 11:53:58 MDT 2022  i686
                          (was 3.12.17 #118, Aug 2018, before the factory
                           restore described below)
toolchain (from /proc/version)
                          gcc 4.8.3, "Control4 Toolchain for Intel CE5300 Series"
cmdline                   console=ttyS0,115200 rw root=/dev/mmcblk0p1 rootwait
                          ip=none memmap=exactmap memmap=128K@128K memmap=1585M@1M
                          vmalloc=586M androidboot.hardware=intelce
```

Board codename **ninjago**, same as EA1.

### The board-ID straps decode

`/dev/gpio/board_id0..6` (gpio17..23) are not an opaque 7-bit board number —
they split into the two values the kernel exports under `/proc/c4board`:

```
board_id0..3  ->  revision      1,0,0,1  = 9   ( = /proc/c4board/revision )
board_id4..6  ->  type          0,1,0    = 2   ( = /proc/c4board/type     )
```

So **type 1 = EA1, type 2 = EA3**, with the low nibble carrying the PCB
revision. This unit is an EA3 rev 9. Worth knowing: a board profile can be
selected on the host from these pins without parsing `/proc/c4board`.

### The factory restore, and why it was worth doing

This unit arrived on a 2018 build (OS 2.10-era, kernel **3.12.17 #118**). It has
since been factory-restored from p2 and now runs **3.12.74 #8-140-ninjago.1**
(May 2022) — the same kernel the EA1 runs, and the same version as the last GPL
drop that covers these boards.

That alignment is the point. `MODVERSIONS` is off, so an out-of-tree module's
`vermagic` string is the only compatibility gate, and `MODULE_FORCE_LOAD` is off
too, so it cannot be overridden. On 3.12.17 there was no matching source to
build against (the GPL drop ships 3.12.74); after the restore there is:

```
running vermagic   3.12.74 SMP preempt mod_unload ATOM
GPL drop           linux-3.12.74 + 54 Intel + 25 Control4 patches
EA1                3.12.74 #8-140-ninjago.1   — identical
```

**The restore itself is now proven on an EA3**, which it was not before. It ran
from the recovery button, extracted `recfs.tar.xz` over p1, and rebooted into a
working stock image in about a minute of post-extract boot. There is a benign
`device_shutdown` warning in the reboot path (a driver's `->shutdown` splats;
the machine restarts anyway). Everything checked afterwards was unchanged:
switch link summary still `0x24`, same IO-MCU images, three user-serial sockets,
DSP firmware present.

Nothing else in this document depends on which build is booted, but readings
were taken across both and version-sensitive claims are tagged.

### Module and boot posture (why a custom kernel is reachable)

Read from `/proc/config.gz` on the restored 3.12.74 — and byte-identical to the
same flags on 3.12.17, so this is a property of the platform, not one build:

```
CONFIG_MODULES=y                       unsigned modules load...
# CONFIG_MODULE_SIG is not set         ...with no signature check
# CONFIG_MODVERSIONS is not set        vermagic is the only gate
# CONFIG_MODULE_FORCE_LOAD is not set  and it cannot be bypassed
CONFIG_KALLSYMS=y                      non-exported core symbols are resolvable
# CONFIG_DEBUG_SET_MODULE_RONX is not set   module pages stay RWX
# CONFIG_STRICT_DEVMEM is not set      /dev/mem reaches all RAM
CONFIG_DEVKMEM=y  CONFIG_DEVPORT=y
# CONFIG_KEXEC is not set              the one thing missing
CONFIG_PHYSICAL_START=0x1000000        vendor kernel loads at 16 MB
# CONFIG_RELOCATABLE is not set
CONFIG_SMP=y  CONFIG_NR_CPUS=8  CONFIG_X86_PAE=y
```

`sys_kexec_load` is present only as the weak `cond_syscall` stub, so there is no
kexec syscall. But unsigned modules load freely, which means the gap is fillable
from a module rather than being a wall.

The load addresses matter more than they look: `bootlinux` places a kernel at
**1 MB** and the running vendor kernel sits at **16 MB**, so a replacement
kernel's destination does not overlap the kernel doing the copying. That removes
the hardest part of a real kexec — relocating the trampoline out of memory it is
about to overwrite while executing from it. A small identity-mapped stub to drop
paging and jump, plus an SMP quiesce across four CPUs, is still required.

`/dev/mem` reads normal RAM but returns `EFAULT` for MMIO above the memmap'd
range (e.g. the `0xdf8f0060` SEC_BOOT register), because `read()` cannot reach
it — that needs an `mmap` helper, and there is no compiler or `devmem` applet on
the box.

## SoC / memory

```
Intel Atom CE5310 @ 1.20GHz — 2 cores / 4 threads, family 6 model 54, stepping 2
MemTotal 1597992 kB (~1.5 GB), zram0 swap (2 GB backing)
```

**Identical to the EA1.** Firmly i686 — the hardware matrix's `i686 ?` for EA3 is
now confirmed, not inferred. ALSA names the audio complex `IntelCE353xx`.

Notable PCI functions (all on the SoC, `lspci -nn`):

```
00:00.0 8086:0c40  host bridge
01:02.0 8086:089b  display   -> PowerVR SGX (pvrsrvkm)
01:16.0 8086:070a  display   -> Vivante GC300 2D (galcore)
01:0b.4 8086:2e6a  SPI master (pxa2xx-spi.0)  -> switch + spidev
01:0c.0 8086:2e6e  Ethernet  (e1000)
01:17.0 8086:08a0  SPI       -> SPI-NOR (nmyx25, mtd0)
01:1b.0 8086:070b  SD host   -> eMMC
01:0d.0-2 192e:0101 EHCI USB x3
```

## Storage

```
/dev/mmcblk0   7.6 GB eMMC          (same layout as EA1)
  p1  6.0G  ext4  /                 rw, noatime, discard, nobarrier
  p2  1.0G        recovery payload  (see below)
  p3   32M  ext4  /mnt/persistent
  mmcblk0boot0/boot1/rpmb
mtd0  16M  "nmyx25"  SPI NOR (spi1.0) — bootloader
tmpfs /tmp 32M, /var 64M
```

`p2` is the factory-restore payload, byte-for-byte the same shape as the EA1's:

```
cefdk.deb            bootloader package        version 36-34
recovery_kernel.deb  boot kernel               3.12.74-8-140
recfs.tar.xz         factory rootfs (490 MB)   3.3.0.628678-res
common.hcfg
manifest             version strings + md5s for all of the above
```

**No GRUB anywhere** — CEFDK loads the kernel directly, same as EA1, so
[bootloader-access.md](bootloader-access.md) and the netboot path should apply
unchanged. *(Not yet tested on EA3 — see "What is not yet verified".)*

The `manifest` also names `garmadon-fpga-45t-spi-rev8.bin`. See the FPGA section.

## Serial map

```
/dev/ttyS0                      console, 115200          (0x03f8)
/dev/ttyS1  = /dev/ttySIO       TI Tiva IO MCU TM4C1231D5 (0x02f8)
/dev/ttyS2                      PIC24 power/watchdog MCU  (0x03e8, only port
                                probing as a real 8250)
/dev/ttyS3                                                (0x02e8)
/dev/ttyUSB0 = /dev/ttySZigbee  Zigbee NCP behind CP2104  (10c4:ea60)
```

Identical to the EA1, including the wrong comment in `/etc/rc.d/99control4`
that calls ttyS2 an "8051 Power Management Inside CE53xx" (it is a PIC24 — see
[hdmi-cec.md](hdmi-cec.md)).

**The three user-facing RS-232 ports are MCU-routed, not host ttys.** `ioserver`
proves the count by the sockets it opens:

```
5100  dt_listen_port (from /etc/ioserver_config.conf)
5101  user serial port 1   \
5102  user serial port 2    >  three combo IR/serial jacks
5103  user serial port 3   /
20000 io_listen_socket
```

EA1 opens 5101/5102 only. This is the cleanest non-invasive way to count user
serial ports on any of these boards.

## IO MCU

```
/dev/ttySIO -> /dev/ttyS1, 460800 baud (NOT 115200 — see io-mcu-firmware.md)
reset: /dev/gpio/io_reset (gpio7)
```

`/control4/firmware/io/.flash.config` maps **ea1, ea3 and ea5 to the same image
pair** — bootloader `1.1.9.b6a901f`, app `1.0.36.20ebb2d`, byte-identical files
to the EA1's. One firmware, six board profiles selected at runtime.

The per-board profile table inside that image has now been fully decoded, and it
identifies which block is which board. See
[io-mcu-firmware.md](io-mcu-firmware.md#the-per-board-profile-table); the short
version for EA3 is **block 3: seven IR outputs and three user UARTs.**

## Radios / IO

```
Zigbee   CP2104 USB->UART (10c4:ea60, cp210x) -> EM357-class NCP
         reset: /dev/gpio/zigbee_reset (gpio29)
         bridge reset: /dev/gpio/usb_2_serial_reset (gpio26)
Z-Wave   ZM5304 module, zwaved, region firmware in /control4/firmware/zwave/
         (serialapi_controller_static_ZM5304_{US,EU,ANZ,CN,HK,IL}.hex)
Wi-Fi    NONE. No ath9k, no /sys/class/ieee80211, no wlan interface.
         The wlan_disable GPIO exists but is shared board support, not a radio.
IO MCU   TM4C1231D5 on ttyS1
```

The Zigbee attach is identical to the EA1 (USB, not an on-SoC UART). The NCP part
number cannot be read from the host — USB only shows the CP2104 bridge — so
"EM357-class" is from the peripheral `.ebl` images Control4 ships, not a direct
read of the die.

### GPIO aliases (`/dev/gpio/`)

EA3 carries everything the EA1 has plus a power/switch group:

```
shared with EA1   io_reset(7) zigbee_reset(29) codec_reset(24) dsp_reset(101)
                  wlan_disable(27) usb_2_serial_reset(26) board_id0..6(17..23)
EA3 additions     gb_eth_reset(5)      gigabit PHY reset
                  gb_sw_reset(8)       BCM53125 switch reset
                  poe_type(54)         PoE class strap
                  ldo_enable(43)  ac_pwr(80)
                  twelve_volt_ok(122)  n_twelve_volt_current_limit(75)
                  n_usb_current_limit(121)
                  n_fpga_reload(57)    present but unused — see below
LEDs              c4::4ball_red, c4::4ball_blue, c4::network,
                  warn::{red,blue,yellow}, mmc0::
```

## Ethernet and the BCM53125 switch

```
eth0        e1000 (PCI 01:0c.0), MAC 00:0F:FF:94:EE:02
            dmesg: "GBE working in Internal Fake Phy Mode"
                   "e1000_copper_link_preconfig: Phy ID = 0x3625f20"
switch      spi-bcm53125 on spi0.1
            dmesg: "spi_bcm53125_probe: BCM53125 SPI Driver"
            resets: gb_sw_reset (gpio8), gb_eth_reset (gpio5)
also up     br0 172.18.0.1/16 + veth  -> the Android LXC container's bridge
            eth_udma0/1                -> SoC DMA pseudo-interfaces
```

This is the one genuinely new subsystem versus the EA1. The e1000 MAC does not
talk to a PHY at all — it runs in "internal fake phy" mode against a fixed link,
and the **BCM53125 managed switch sits behind it on SPI, not MDIO**. The two
external RJ45s are switch ports; the CPU is the switch's third port.

### The switch is fully inspectable from the running unit

The vendor driver exports a raw register window plus per-port counters:

```
/sys/bus/spi/devices/spi0.1/port      rw   port selector (0-5 and 8; >5 clamps to 8)
/sys/bus/spi/devices/spi0.1/page_reg  rw   "0x01 0x00" style page/register selector
/sys/bus/spi/devices/spi0.1/value     r    reads the selected register
/sys/bus/spi/devices/spi0.1/mibs      r    full MIB counter block for the selected port
```

Reading is harmless; **do not write `value`** on a unit you reach over the
network, because the path to it runs through this switch.

That is enough to map the ports, and the map is not the obvious one:

```
page 0x01 reg 0x00  Link Status Summary = 0x24   -> ports 2 and 5 up
page 0x00 reg 0x5d  port 5 GMII override = 0x4b  -> forced link, 1000M, full duplex
per-port MIBs       port 2 and port 5 are exact mirrors:
                      port 2  TxOctets 1,496,956  RxGoodOctets 1,376,978
                      port 5  TxOctets 1,403,152  RxGoodOctets 1,499,259
```

The map was then settled by a **controlled traffic test** rather than inference:
snapshot all ports, push ~4 MB out of the box, snapshot again.

```
port 5   dRxGood = 4,313,853     <- switch receives it from the SoC MAC
port 2   dTx     = 4,313,811     <- switch sends it out the jack
ports 0,1,3,4    zero
```

A 42-byte discrepancy across 4 MB is framing. That is direct proof, not a
correlation.

| switch port | role | evidence |
|---|---|---|
| **5** | **CPU port** to the SoC e1000 | 4.3 MB delta inbound during the push; forced 1000/full override |
| **2** | primary rear RJ45 | 4.3 MB delta outbound during the push; link up with a partner |
| **1** | second rear RJ45, **isolated** | links and receives, but never forwards — see below |
| 0, 3, 4 | unused | no link, no counters, no delta |
| 8 (IMP) | **not** the CPU port here | no link, all-zero counters |

Ports 0–4 all report the same integrated GPHY (`0362:5f24`) — they are the
BCM53125's five built-in PHYs — so silicon presence cannot distinguish a routed
jack from an unrouted one. Port 1 was identified by moving the cable instead.

### The second jack is live but not bridged to the CPU

Moving the cable to the other RJ45 makes the unit **completely unreachable**,
and that is not a dead port. While the cable sat there, port 1's counters told
the whole story:

```
RxOctets / RxGoodOctets : 1,455,777      TxOctets : 0
RxMulticastPkts         : 4,565
RxBroadcastPkts         : 224
RxUnicastPkts           : 23
```

4,812 frames averaging 302 bytes — ordinary LAN flood. So the PHY links, the
port receives, and the switch **transmitted exactly zero bytes out of it** while
the host never answered. The stock switch configuration isolates that jack from
the CPU port.

The vendor driver's register window will not confirm the mechanism — it returns
"Not Supported or Not Implemented" for the VLAN pages (`0x31`, `0x34`), so the
port-based VLAN map cannot be read through it.

**This is the argument for the DSA work.** Without a switch driver the second
RJ45 is unusable, and no amount of `e1000` configuration changes that.

> Counter caution: these MIBs are cumulative, not clear-on-read (a port with no
> link holds its value across repeated reads), but they **do** reset on reboot,
> and this unit rebooted mid-investigation. Take a snapshot immediately before
> and after any experiment rather than trusting older numbers.

Consequences for an open image:

* The vendor `spi-bcm53125` driver is out-of-tree. Mainline's equivalent is the
  **DSA `b53` driver**, which does support the BCM53125 and does have an SPI
  binding (`b53_spi.c`), so this is a *portable* subsystem — unlike the graphics
  stack. It needs a device-tree/board description, which is the same de-DT
  problem already solved for `i2c-pxa` in `board/ea-common/patches/linux/0001-*`.
* Until the switch is brought up, a mainline kernel should still get basic
  networking through `e1000` on the fake-phy link, because the switch powers up
  passing traffic. **Unverified** — worth being the first thing tested.

## I²C / SPI device map

```
i2c-0  (pxa_i2c)   0x48  lm75    temperature sensor   -> already handled by the
                                                         EA1 lm75 patch
                   0x68  ds1339  RTC (rtc-ds1307)
i2c-1  (pxa_i2c)   -
i2c-2  (pxa_i2c)   -
i2c-3  (pxa_i2c)   0x38  adau1451  SigmaDSP

spi0.1  spi-bcm53125   managed switch
spi0.2  spidev         exported to userspace
spi0.3  spidev         exported to userspace
spi1.0  nmyx25         SPI NOR -> mtd0
spi1.1  (unbound)
```

### The AK4621 is not software-controlled at all

The **AK4621EF has no control path on this system**, and that is now a positive
finding rather than a gap. Four independent checks:

```
grep -c ak4621 /proc/kallsyms          -> 0     no symbol anywhere in the kernel
grep ak46 /proc/modules                -> none  no module
readlink /proc/*/fd/* | grep spidev    -> none  nothing has spidev0.2 or 0.3 open
strings /control4/bin/* | grep spidev  -> none  no vendor binary mentions spidev
```

`ea3_dsploader` references exactly one device, `/dev/i2c-3` — the ADAU1451. The
only thing anyone does to the codec is release its reset at boot:

```
/etc/rc.d/04c4_gpio:  setup_gpio 24 high codec_reset
/etc/rc.d/10adau_dsp: ea3_dsploader -l -v -20    (per board: ea3/ea5/tr1)
```

So the part is **strapped into standalone/hardware mode**: reset is deasserted,
I²S1 feeds it, and nothing ever configures it over a control port. The two
`spidev` nodes are exported but unused by the stock system.

For an open image this is good news — the codec needs **no driver**, only
`codec_reset` high and I²S1 running. What remains unknown is merely what the
unused `spidev0.2`/`spidev0.3` are wired to, which no longer blocks audio.

## Audio

Two ALSA cards. Card 0 is the SoC's Intel SMD audio; card 1 is the SigmaDSP.

```
card 0 [IntelCE353xx]
  device 0  analog0    -> line out (via AK4621)
  device 1  digital0   -> S/PDIF coax out
  device 2  hdmi0      -> HDMI audio
  every device exposes 4 subdevices: nav, stream, intercom, announce
card 1 [ninjagosmdadau1  "ninjago-smd-adau1451"]
  device 0  SMD DAI PCM adau1451-ch0-0
```

**No capture devices at all** (`arecord -l` is empty, `/proc/asound/pcm` lists
only playback). The line-in and coax-in jacks are therefore *not* reachable
through ALSA on the stock image — they are wired into the ADAU1451, which mixes
them internally under DSP program control. Anything that wants them as a capture
source has to go through the DSP.

Mixer controls:

```
card 0   SpeakerMap Channels Encoding Passthrough
         Analog0 Digital0 HDMI0  Input x3  OutputParams x3
card 1   Master Bass Treble Balance Loudness VolumeCurve
         InputGain Input OutputMode OutputParams FirmwareRate
         Filter31_5 Filter63 Filter125 Filter250 Filter500
         Filter1000 Filter2000 Filter4000 Filter8000 Filter16000
```

Card 1's controls are a full 10-band graphic EQ plus tone/loudness — that is the
ADAU1451 program, not a generic codec.

### DSP firmware

```
/lib/firmware/ea3-1451.bin    80,040 B   <- this board
/lib/firmware/ea5-1451.bin   152,392 B
/lib/firmware/tr1-1451.bin    99,560 B
loader: /control4/bin/ea3_dsploader   (also ea5_dsploader, tr1_dsploader)
kernel: snd_soc_adau1451 + snd_soc_adau1451_common + snd_soc_sigmadsp_{i2c,new}
dmesg:  adau1451 3-0038: sigmadsp_firmware_load: ### Load Firmware v2
        ninjago-smd-codec.1: adau1451-ch0 <-> ninjago-smd-dai.0 mapping ok
reset:  /dev/gpio/dsp_reset (gpio101)
```

These are SigmaStudio program images. A copy of `ea3-1451.bin` has been pulled;
it is vendor firmware, so it is **not** committed to this repo — see
`firmware/dsp/adau1451/README.md`.

### Output routing config

`/etc/hdmi_hpd.cfg` documents the fixed output preferences:

```
spdif_audio_pref   enabled=1, 96 kHz / 24-bit
                   "optical and coaxial spdif are the same output"
hdmi_audio_pref    PCM, ENCODED_DTS, ENCODED_DD, ENCODED_AAC; up to 8ch; 192 kHz
hdmi_video_pref    1920x1080 / 1280x720 / 720x576 / 640x480 @ 59.94, 50, 60
i2s_audio_pref     enabled=2  -> i2s1 only  (the SoC <-> ADAU1451 link)
```

## There is no FPGA on this board

The EA3 rev 9 PCB has no FPGA, and the software agrees — but only if you look
past the driver list, which is misleading:

```
loaded modules   snd_ninjago_fpga, snd_ninjago_fpga_pcm,
                 snd_soc_ninjago_fpga_dai, snd_soc_ninjago_fpga_dsp
registered drvs  pci:ninjago-fpga, platform:ninjago-fpga-dai,
                 platform:ninjago-fpga-codec, platform:snd-ninjago-fpga
bound devices    NONE — every one of those driver directories contains only
                 bind/unbind/uevent, with no device symlinks
GPIO             /dev/gpio/n_fpga_reload (gpio57) exists and reads 1
manifest         names garmadon-fpga-45t-spi-rev8.bin
```

So: the "ninjago" kernel is one build shared across the EA family, the FPGA
audio drivers ship in it unconditionally, and on an EA3 nothing probes. The FPGA
image is `garmadon-*` — **garmadon is the EA5's board codename**, which is where
that hardware lives. On EA3 the ADAU1451 does the job the FPGA does on an EA5.

Practical upshot: `snd_ninjago_fpga*` in an `lsmod` is not evidence of an FPGA.
Check `/sys/bus/*/drivers/ninjago-fpga*/` for bound devices instead.

## Graphics / HDMI — same closed wall as the EA1

```
GPU        PowerVR SGX on PCI 01:02.0
           pvrsrvkm 1.12.3052601, "SGX revision = 1014", sgx_intel_ce DDK
2D         Vivante GC300 on PCI 01:16.0 (galcore) — /etc/directfbrc_GC300
display    gdl_server + /bin/gdl_udaemon, pd_hdmi, pd_inttvenc_cvbs
media      the full Intel SMD stack (ismdcore, ismdviddec_v3, ismdvidenc,
           ismdaudio, ismddemux_v3, hdmi_rx_ce, vidcap_ce4X00, ...)
nodes      /dev/dri/card0, /dev/gdl/0, /dev/gdl/track, /dev/pvr_sync, /dev/galcore
```

**Answering the "is there a 7.1.8-era driver for this GPU?" question: no.**
`/dev/dri/card0` is the PowerVR DDK's own DRM shim, not KMS. The part is a
**Series5 SGX**, and mainline has never carried an SGX driver — the `powervr`
DRM driver merged in 6.8 is for **Rogue (Series6+)** and does not cover SGX. The
`gpu/drm/imx`/`etnaviv` route does not apply either: `etnaviv` targets Vivante,
and while the GC300 2D core here *is* Vivante, etnaviv supports GC-series 2D/3D
cores over a device-tree binding and has never been tried against this
PCI-attached CE5300 variant.

So the EA3 is in exactly the same position as the EA1: HDMI means keeping the
3.12 kernel and the GPL `gdl_server`/`pd_hdmi`/`ismd*` modules plus the
proprietary userspace. A mainline kernel on EA3 loses video, same as EA1. There
is no EA3-specific escape hatch.

## Android UI

Same as EA1 — Android runs in an **LXC container** named `android`, bridged on
`br0` (172.18.0.1/16) with a veth pair:

```
lxc-start -F --pidfile=/var/run/lxc.pid --name=android
inside: zygote, system_server, surfaceflinger, servicemanager, drmserver,
        mediaserver, netd, vold, logd, adbd,
        com.control4.android.launcher, com.control4.app,
        com.control4.android.c4settings, com.control4.deviceadministrator,
        com.control4.android.magicsmoke.custom, anysoftkeyboard
host bits: /dev/binder, /dev/ashmem, logger, timed_gpio, /dev/log*
```

## Stock userland

Busybox/SysV init plus Control4's `sysmand`. Running daemons:

```
director c4server ioserver sysmand watchdogd c4faultd led_service
audio3server audio3clock audio3streamer spotifyd spotifyclient shaird
vidmand imaged upmand netusbserver raproxyd c4rmengined c4lookup sddpd
remote_keyd zwaved zserver broker(node.js) nginx atftpd snmpd smbd nmbd
ntpd dropbear dhclient ipwatchd bluetoothd dbus-daemon syslog-ng
```

`spotifyclient -n Speakerpoint` — the EA3 registers its audio endpoint as a
Speakerpoint.

## What is not yet verified

Everything above was read off the box. These were **not** tested and must not be
treated as done:

* **Netboot / CEFDK shell on EA3.** The boot chain looks identical to EA1
  (CEFDK 36-34, no GRUB, same recovery layout) but no EA3 has been netbooted,
  and `SEC_BOOT` has not been read on this unit. Do not assume unsigned kernels
  boot here just because they do on the EA1.
* **Which physical rear jack maps to which IR channel index.**
* **The BCM53125 under mainline `b53`/DSA**, and whether `e1000` alone gets a
  link on a mainline kernel.
* **Which switch port is the second RJ45.** Port 5 (CPU) and port 2 (a jack)
  are measured; port 1 is inferred from stale counters. Settle it by moving a
  cable and re-reading `page 0x01 reg 0x00`.
* **The Zigbee NCP part number** (EM357 vs the EM537 marking on the PCB).
* **The board-ID strap voltage.** The MCU's selector arithmetic is fully
  decoded, which *predicts* ~0.60 V on PB4 for an EA3 and ~0.40 V for an EA1 —
  but no one has measured it. See io-mcu-firmware.md.
* **Which of the MCU's two four-pin groups is relays and which is contacts.**
  EA3 populates `PF0` and `PA2`, one from each group; nothing distinguishes an
  input group from an output group in the table.

Closed since the first pass:

* the AK4621 control path — there is none, the part is strapped standalone
* the switch's CPU port (**5**, not the IMP at 8), the primary jack (**2**) and
  the second jack (**1**, isolated by the stock configuration)
* the MCU board-ID selector arithmetic
* **the contact's wire semantics** — `CONTACT_GET` is a u32 bitmask, EA3's
  contact is bit 0, closed reads 1, and the host must poll for it. Measured by
  shorting the input; see
  [io-mcu-firmware.md](io-mcu-firmware.md#contacts--confirmed-on-an-ea3).

## Reproduce

```bash
tools/ssh 10.0.0.139 'cat /proc/c4board/name /proc/c4board/revision'
```

`tools/ssh` already carries the legacy kex/cipher/MAC flags this dropbear needs.
