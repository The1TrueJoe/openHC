---
title: EA-1 recon
description: A live pull off a running EA-1 — SoC, storage, serial map, radios, graphics and the stock userland.
sidebar:
  order: 2
---

Everything below came off a running EA-1 over SSH as root. Control4 ships the
same password on every unit, so getting in is not the hard part.

:::caution[Check what you are actually looking at]
This is the only first-party EA-1 data we have. An `ea1_remote/` capture floating
around in the research folder turned out to be an **HC-800**, byte-identical
HC800 binaries, mislabelled. Worth knowing before you trust a dump someone hands
you.
:::

## Identity

```
/proc/c4board/name      ea1
/proc/c4board/revision  5
/proc/c4board/type      1   (binary 001)
hostname                ea1-000FFF1AFCA9      (MAC 00:0F:FF:1A:FC:A9)
uname                   Linux 3.12.74 #8-140-ninjago.1 SMP PREEMPT ... i686
```

Board codename **ninjago**, `androidboot.hardware=intelce`. Debian userland
(`dpkg`/`apt`, per-board `product-manifest`).

## SoC and memory

```
Intel Atom CE5310 @ 1.20GHz — 2 cores / 4 threads, family 6 model 54
MemTotal 1597780 kB (~1.5 GB), zram0 swap
get_soc_info_utility NAME → SOC_NAME_CE5300
Toolchain: i586-control4-linux-gnu, gcc 4.8.3, glibc 2.19
```

Firmly **i686**, not ARM.

## Storage

```
/dev/mmcblk0   7.6 GB eMMC
  p1  6.0G  ext4  /            (rootfs, rw, discard, data=ordered)
  p2  1.0G                     recovery payload
  p3   32M  ext4  /mnt/persistent
  mmcblk0boot0/boot1/rpmb
mtd0  16M  "nmyx25"            SPI NOR — bootloader/env
```

## The serial map

This is the important one, and it differs from the HC-800 in two ways that
matter.

```
/dev/ttyS0                      console, 115200
/dev/ttyS1  = /dev/ttySIO       TI Tiva IO MCU (TM4C1231D5)
/dev/ttyS2                      PIC24 watchdog/power MCU — not the IO chip
/dev/ttyS3
/dev/ttyUSB0 = /dev/ttySZigbee  Zigbee NCP behind a CP2104 USB-UART bridge
```

The IO MCU is on **ttyS1**, not ttyS3, and Zigbee is **USB**, not an on-SoC UART.

The two user-facing RS-232 ports are **MCU-routed, not host ttys**. The CP2104
bridge exposes only Zigbee, the vendor's own udev script links interface 00 to
`/dev/ttySZigbee` and nothing else.

## Radios and IO

```
Zigbee   CP2104 USB→UART (10c4:ea60, cp210x) → EM357-class NCP
         reset: /dev/gpio/zigbee_reset (gpio29)
Wi-Fi    Atheros AR9485 at PCI 0000:03:00.0, ath9k — no firmware blob
         disable: /dev/gpio/wlan_disable (gpio27)
IO MCU   TM4C1231D5 on ttyS1 @ 460800, reset /dev/gpio/io_reset (gpio7)
```

The stock image already loads ath9k and binds it at boot; `wlan0` simply sat
DOWN and unconfigured, because **Control4 never used Wi-Fi on this model**. There
is no vendor Wi-Fi script to replace.

Three things worth knowing about the radio:

- **Wi-Fi bring-up doesn't trip the watchdog.** An earlier note claimed every
  `wlan0` bring-up hard-rebooted the box. Each step was re-run with an uptime
  guard around it, link up, scan, associate, DHCP, with zero reboots. The
  original symptom was most likely the vendor net-watchdog reacting to a route
  change.
- **`/dev/gpio/wlan_disable` is irrelevant.** It reads `1` at boot and the radio
  works anyway; it does not gate the radio.
- **There is no `udhcpc`.** The DHCP client is ISC `dhclient`, and its `-timeout`
  is a config-file option, not a CLI flag.

## Zigbee: the bootloader trap

`ohc-serialbridge` puts the UART on the network and works, but the NCP answers
nothing, not EZSP/ASH, at any baud, with or without RTS/CTS, not even after a
hardware reset with the port already open.

Running the vendor daemon explains it:

```
ZbRadio::hardwareResetPanelToBootloader: Stopping EZSP interface ...
NcpInterface::executeResetScript: Hardware reset of the front-panel
ZapServer::shutdownNcp: NCP should be in bootloader
```

**The radio is in the EM357 serial bootloader, not running EmberZNet.** There's
no EZSP to talk to because the application isn't running. The vendor puts it
there deliberately and would normally boot it back.

The sequence that works, all of it matters:

1. Open the port at **115200 8N1, no RTS/CTS**. The vendor's `stty 1cb2` decodes
   to `B115200|CS8|CREAD|CLOCAL|HUPCL` — **no `CRTSCTS`**. With RTS/CTS on, the
   kernel blocks every write waiting for CTS and the radio looks dead.
2. Pulse reset: `echo 0 > /dev/gpio/zigbee_reset; sleep 1; echo 1 > ...`
3. Send `\r\n` a few times → the bootloader menu appears:
   `EM357 Serial Bootloader v45 bA8 / 1. upload ebl / 2. run / 3. ebl info / BL >`
4. Send **`2`** → the app starts and immediately emits an ASH RSTACK.

Verified:

```
'2'        -> ff 1a c1 02 09 2a 10 7e     app booted, RSTACK
ASH reset  -> 1a c1 02 0b 0a 52 7e        0xC1 = RSTACK, ash v2, reset code 0x0b
```

Once running, `tcp://<host>:6638` takes zigbee2mqtt (`port: tcp://...`) or ZHA
(`socket://...`).

### We never need to ship the firmware image

Worth being explicit, because it removes a licensing problem entirely: **the NCP
firmware is already flashed on every unit's radio.** Our software only tells the
existing image to *run*. We do not copy, bundle or redistribute the `.ebl`, it
stays on the user's own device, exactly like the graphics blobs.

The only case needing an image is re-flashing a radio whose app is lost. In
order of preference: the `.ebl` already present on that same device
(`/control4/firmware/pro/`, the user's own file); a stock Silicon Labs EM357 NCP
image under SiLabs' terms; never Control4's copy bundled into our releases.

Whether the on-device `.ebl` is byte-identical to a stock SiLabs build is
unconfirmed, the filename follows SiLabs' convention and the header says
`ZNCPVer:4720`, but since path one needs no redistribution, that does not block
anything.

## Fan and thermal — fully open

Both ends are standard interfaces; no vendor code:

```
temp in : /sys/class/hwmon/hwmon0/device/temp1_input   LM75 on I2C, milli-C
          temp1_max = 80000  (the vendor's own critical limit)
fan out : /sys/class/pwm/pwm0:2/{period_ns,duty_ns,run}
          period_ns = 40000 (25 kHz); duty_ns = period * pct/100
```

Two non-obvious properties of that PWM interface. **`run` is write-only** —
reading it returns `EACCES`, and that is normal. And the channel must be
**claimed** first (`echo 1 > request`); until claimed, writes to `period_ns` are
silently dropped, the period stays 0, and the fan cannot spin at any duty. The
claim is **per-process** and dies with its owner.

The stock curve, from `/etc/c4faultd.conf`: `on_temp 50`, `min_on_pcnt 10`,
`pcnt_per_degree 10`, `run_time_after_cool_min 60`, plus an 80 °C critical
override and a fail-safe to 100 % if the sensor read fails.

Idle reference: heatsink ~42 °C, cpu0 ~51 °C, fan 0 %, correct, because it is
below 50 °C.

Note the CPU temperatures come from `/dev/thermal`, an ioctl-based Intel CE char
device, and are **not** what the fan curve uses. Mainline `coretemp` and
`x86_pkg_temp_thermal` both fail to bind on the CE5310, so the LM75 heatsink
sensor is the open path.

## The PIC24 watchdog

`/dev/ttyS2` goes to a **Microchip PIC24**, a discrete part on the board, not
the 8051 inside the SoC that CEFDK reports at boot. Intel's own diagnostic tool
settles the naming:

```
$ strings /usr/dtsbin/pic24
The uart device is wrong, use '-dev /dev/ttyS2' parameters for CE4200 and CE5300 and CE2600
InitPIC24()
PIC24 Version:%s
```

Control4's own `/etc/rc.d/99control4` comments the port as "8051 Power Management
Inside CE53xx", which is **wrong for this board**; their own `watchdogd` prints
`PIC24 Version`. The kernel agrees something is different: it enumerates as a
plain `8250` at `0x3e8` while `ttyS0/1/3` are all `GEN3_serial`, the SoC's own
UART block.

The PIC owns more than the watchdog. Intel's `libpicuart.so` exposes
`setGPIOValue`, `setPWM`, `setIrRepeatMode`, a `PicBufferIR`, and a full
[HDMI-CEC message class](/ea/hdmi-cec/). Framing is ASCII-hex with an XOR
checksum:

```
AA <n> <body as hex> <xor as hex>     n = hex chars in body
ack  AA 02 "0606"     nak  AA 02 "0707"     CEC ack  AA 04 "050005"
```

Traffic is one `cmd(26)` heartbeat every ten seconds and nothing else.
**Replacing `watchdogd` means owning that link**, heartbeat on time, every time.
Miss it and the board resets within a minute. Until then the vendor daemon stays;
it is the last Control4 process running.

## Graphics and HDMI

```
Modules: gdl_server, gdl_udaemon, pd_hdmi.ko, ismd* (Intel SMD media),
         galcore + pvrsrvkm (IMG PowerVR SGX GPU), pd_inttvenc_cvbs
Nodes:   /dev/dri/card0 (PowerVR, not Intel KMS), /dev/gdl/0, /dev/pvr_sync
```

HDMI is driven by Intel's **GDL** stack, not DRM/KMS. `/dev/dri/card0` is the
PowerVR DDK's own DRM shim. See [graphics](/ea/graphics/), the GPU itself
turned out to be reachable on a mainline kernel.

## Audio

`ADAU1451` SigmaDSP plus Intel ISMD audio and `c4audiosense`. Not plain Intel HDA
— that's the HC-800. See [audio](/ea/audio/).

## Android

Android runs in an **LXC container** named `android`, with `zygote`,
`system_server`, `surfaceflinger`, `servicemanager` and the Control4 launcher
inside it. SurfaceFlinger renders straight onto the GDL/HDMI plane.

Keeping this UI means keeping the host binder/ashmem kernel bits, the container,
and the Intel GDL modules the container's gralloc talks to, and the container
itself is Control4's AOSP build, which the clean-room rule says we don't
redistribute.

## Stock services an open image replaces

SysV/busybox init plus Control4 `sysmand`. Running daemons: `director`,
`sysmand`, `led_service`, `watchdogd`, `smbd`/`nmbd`, `ntpd`, `dropbear`, plus an
OvrC cloud agent connecting to `wss://cloud.ovrc.com`.

## Reproduce

Dropbear on these units needs legacy algorithms:

```bash
ssh -o KexAlgorithms=+diffie-hellman-group14-sha1 \
    -o HostKeyAlgorithms=+ssh-rsa -o Ciphers=+aes128-cbc \
    root@<ip>
```
