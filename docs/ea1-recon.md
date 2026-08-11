# EA1 live recon

Everything below came off a running EA-1 over SSH as root. Control4 ships the
same password on every unit, so getting in is not the hard part.

This is the only first-party EA-1 data we have. An `ea1_remote/` capture floating
around in the research folder turned out to be an HC800 — byte-identical HC800
binaries, mislabelled. Worth knowing before you trust a dump someone hands you.

## Identity

```
/proc/c4board/name      ea1
/proc/c4board/revision  5
/proc/c4board/type      1   (binary 001)
hostname                ea1-000FFF1AFCA9      (MAC 00:0F:FF:1A:FC:A9)
uname                   Linux 3.12.74 #8-140-ninjago.1 SMP PREEMPT ... i686
cmdline                 console=ttyS0,115200 ... androidboot.hardware=intelce
```

Board codename **ninjago**, `androidboot.hardware=intelce`. Debian userland
(`dpkg`/`apt`, per-board `product-manifest`).

## SoC / memory

```
Intel Atom CE5310 @ 1.20GHz — 2 cores / 4 threads, family 6 model 54
MemTotal 1597780 kB (~1.5 GB), zram0 swap
get_soc_info_utility NAME → SOC_NAME_CE5300
Toolchain (from an HC800 sibling binary): i586-control4-linux-gnu, gcc 4.8.3, glibc 2.19
```

Firmly **i686**, not ARM.

## Storage

```
/dev/mmcblk0   7.6 GB eMMC
  p1  6.0G  ext4  /            (rootfs, rw, discard, data=ordered)
  p2  1.0G
  p3   32M  ext4  /mnt/persistent
  mmcblk0boot0/boot1/rpmb
mtd0  16M  "nmyx25"  (SPI NOR — bootloader/env)
```

## Serial map (this is the important one)

```
/dev/ttyS0                      console, 115200
/dev/ttyS1  = /dev/ttySIO       TI Tiva IO MCU (TM4C1231D5)
/dev/ttyS2                      PIC24 watchdog/power MCU (not the IO chip) — see hdmi-cec.md
/dev/ttyS3
/dev/ttyUSB0 = /dev/ttySZigbee  Zigbee NCP behind a CP2104 USB-UART bridge
```

Note vs HC800: the IO MCU is on **ttyS1**, not ttyS3, and Zigbee is **USB**, not
an on-SoC UART.

## Radios / IO

```
Zigbee   CP2104 USB→UART (10c4:ea60, cp210x driver) → EM357-class NCP
         reset: /dev/gpio/zigbee_reset (gpio29)
Wi-Fi    Atheros ath9k (open mac80211 driver — no firmware blob)
         disable: /dev/gpio/wlan_disable (gpio27)
IO MCU   TM4C1231D5 on ttyS1, reset /dev/gpio/io_reset (gpio7)
```

GPIO aliases of interest (`/dev/gpio/`): `io_reset`, `zigbee_reset`,
`codec_reset`, `dsp_reset`, `wlan_disable`, `board_id0..6`, plus 4-ball and
warn LEDs under `/sys/class/leds/`.

## Graphics / HDMI (the closed part)

```
Modules: gdl_server, gdl_udaemon, pd_hdmi.ko, ismd* (Intel SMD media),
         galcore + pvrsrvkm (IMG PowerVR SGX GPU), pd_inttvenc_cvbs (composite)
Nodes:   /dev/dri/card0 (PowerVR, not Intel KMS), /dev/gdl/0, /dev/gdl/track,
         /dev/gdl/... , /dev/pvr_sync
Init:    /etc/init.d/{graphics,display,hdmi,directfb}
```

HDMI is driven by Intel's **CEFDK / GDL** stack (`gdl_udaemon`), not DRM/KMS.
`/dev/dri/card0` is the PowerVR SGX GPU. This is the closed-source wall for any
"open" video path — see the bring-up doc.

## Audio

```
ADAU1451 SigmaDSP (snd_soc_adau1451*) + Intel ISMD audio + c4audiosense
```

Not plain Intel HDA (that's the HC800). The ALSA `hw:` map must be read off the
device (`aplay -l`, `amixer scontrols`) before writing an outputs.conf.

## Android UI (kept, per the plan)

Android runs in an **LXC container** named `android`:

```
/etc/init.d/lxc_android        (lxc-stop -kn android on shutdown)
lxc-start / lxc-stop present; /etc/lxc/default.conf
Host kernel has /dev/binder, /dev/ashmem, /dev/ion-style bits, logger, timed_gpio
Inside the container: zygote, system_server, surfaceflinger, servicemanager,
  drmserver, mediaserver, com.control4.android.launcher, com.control4.c4settings,
  anysoftkeyboard, com.android.systemui/settings/phone
```

So SurfaceFlinger renders straight onto the GDL/HDMI plane. Keeping this UI means
keeping: the host binder/ashmem kernel bits, the LXC container, and the Intel GDL
modules the container's gralloc/SF talk to.

## Stock services (what an open image replaces)

Host SysV/busybox init + Control4 `sysmand`. Userland daemons seen running:
`director`, `sysmand`, `led_service`, `watchdogd`, `smbd`/`nmbd`, `ntpd`,
`dropbear`, plus an OvrC cloud agent (`/opt/ovrc-fob/...runtime wss://cloud.ovrc.com`).

## Reproduce

```bash
ohc discover                  # find the box
# then over ssh (legacy kex/cipher needed for dropbear):
ssh -o KexAlgorithms=+diffie-hellman-group14-sha1 \
    -o HostKeyAlgorithms=+ssh-rsa -o Ciphers=+aes128-cbc \
    root@<ip>   # password: t0talc0ntr0l4!
```
