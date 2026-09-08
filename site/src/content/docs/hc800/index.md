---
title: HC-800
description: A live pull off an HC-800 rev 4 — the outlier of the lineup, and the cheapest board here to bring up.
sidebar:
  order: 1
  label: Overview & recon
---

Everything below came off a running **HC-800 (board revision 4)** over SSH as
root.

The HC-800 is the odd one out in this tree, in the direction that helps. **It's
not an embedded SoC board at all — it's a small x86 PC.** Lite-On motherboard,
AMI BIOS, Intel Atom D525 + NM10, SATA SSD, GRUB 0.97, with the Control4
peripherals hung off ISA-range 8250 UARTs and the ICH GPIO block. Every part of
the boot chain is stock PC, nothing is signed, and the bootloader reads a plain
text config off an ext3 partition we can already mount over SSH.

## Identity

```
/proc/c4board/name          hc800
/proc/c4board/revision      4
/proc/c4board/type          0   (binary 000)
hostname                    Main-HC800-000FFF57B978
uname                       Linux 3.16.38-8.260.24 #8.260.24-hc800.2 SMP PREEMPT i686
toolchain (/proc/version)   gcc 4.8.3 — "Control4 Toolchain for Intel CE5300
                            Series", the EA toolchain, reused
eth0 MAC                    00:0f:ff:57:b9:78
wlan0 MAC                   44:6d:57:10:c7:aa   (from the radio's EFUSE)
```

DMI, which no other board in this tree has at all:

```
sys_vendor       Lite-On Tech.
product_name     HC800          product_version  X01
board_vendor     Lite-On Tech.  board_name       AE100
bios_vendor      American Megatrends Inc.
bios_version     0.00.17        bios_date        12/20/2011
```

**No codename.** The EA boards are `ninjago`/`garmadon`, the IOX is `hammer`, the
CA-1 is `emmet`; the HC-800 is just `hc800` everywhere, in `/proc/c4board`, in
the filesystem labels, in the kernel version string, and in `.flash.config`. The
Lego naming convention starts after this generation. (The hardware matrix
previously listed `sherman`; nothing on a live unit uses that name.)

### `type` is 0, and the revision comes from GPIO straps

The vendor's `/etc/init.d/gpio` exports three ICH GPIO lines named `board_id0..2`,
and they read:

```
board_id2 = 1   board_id1 = 0   board_id0 = 0     ->  100b = 4
```

which is exactly `/proc/c4board/revision`. So on this board the three straps carry
the **revision**, not a board-type selector — unlike the EA family, where a single
strap voltage into the IO-MCU's ADC picks the board type.

## The machine

```
Intel Atom D525 @ 1.80GHz (Pineview), family 6 model 28 stepping 10
2 cores / 4 threads, 512 KB L2
MemTotal 2060912 kB (2 GB)
```

Chipset:

```
00:00.0 8086:a000   Pineview host bridge
00:02.0 8086:a001   GMA 3150 (IGD)
00:1b.0 8086:27d8   ICH7 HD Audio
00:1d.7 8086:27cc   ICH7 EHCI (+ four UHCI companions)
00:1f.0 8086:27bc   NM10 / ICH7 LPC
00:1f.2 8086:27c1   ICH7 SATA, AHCI mode
00:1f.3 8086:27da   ICH7 SMBus (i801)
02:00.0 10ec:8168   Realtek RTL8111/8168 GbE
```

Everything on that list is ordinary, decade-old, first-class mainline hardware.
There is no custom silicon anywhere in the chipset, the Control4-specific parts
are all hung off the LPC UARTs, the SMBus and the ICH GPIO block.

### The CPU is 64-bit and the vendor did not use it

`/proc/cpuinfo` includes **`lm`** and `nx`. The vendor nevertheless ships a
**32-bit, non-PAE** kernel:

```
Notice: NX (Execute Disable) protection cannot be enabled: non-PAE kernel!
CONFIG_HIGHMEM4G=y
1149MB HIGHMEM available.   887MB LOWMEM available.
```

so their 2 GB is split across a highmem boundary and NX is off. **openHC builds
this board x86_64.** It is the only board in the tree where we deliberately don't
match the vendor's architecture, and the `lm` flag is the whole justification.

## Storage and the partition layout

```
ATA SanDisk SSD U100 8GB on ata1 (AHCI, SATA 3.0 Gbps)
sda   7824600 blocks (7.46 GiB)
```

`ahci` reports two ports populated: `ata1` is the internal SSD, `ata2` is the rear
**eSATA** jack.

| Part | Size | FS | Role |
|---|---|---|---|
| `sda1` | 10.3 MB | ext3 | **GRUB 0.97** — `stage1`, `stage2`, `menu.lst`, `device.map`. 5.3 MB free. |
| `sda2` | 1.0 GB | ext3 | `restore_fs_hc800` — the factory-restore root, with its own `/boot/bzImage`. |
| `sda3` | 200 MB | ext3 | **kernel-only** partition for the main image. 165 MB free. |
| `sda4` | 6.4 GB | ext4 | `root_fs_hc800` — the running vendor root. |

The reason for the odd `sda3` is worth stating, because it's also why our install
is easy: **GRUB 0.97 can't read ext4 extents**, so the kernel can't live on the
ext4 root. Control4 solved that by giving the kernel its own small ext3 partition
— which has 165 MB free and which GRUB can read every byte of.

## Boot chain: AMI BIOS → GRUB 0.97 → bzImage

`/boot/grub/menu.lst` on `sda1`, verbatim:

```
serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1
terminal serial

support_factorydefault	1
factorydefault		0
default			1
fallback		1
timeout			0
hiddenmenu

title		HC-800 Factory Default Image
root		(hd0,1)
kernel		/boot/bzImage root=restore_fs_hc800 type=ext3 ro console=ttyS0,115200 quiet

title		HC-800 Image
root		(hd0,2)
kernel		/boot/bzImage root=root_fs_hc800 type=ext4 ro console=ttyS0,115200 quiet consoleblank=0 vt.cur_default=1
```

Points that matter for the port:

- **Nothing is verified.** No secure boot, no signed kernel, no measured boot, no
  container format. GRUB `kernel` loads a bare bzImage. This is the least
  locked-down boot chain of any board in this repository.
- **`initrd` works.** Confirmed by string-dumping the installed `stage2`: it's
  GNU GRUB 0.97 and carries the `initrd FILE [ARG ...]` builtin and the
  `[Linux-initrd @ 0x%x, 0x%x bytes]` loader message. So an external
  `rootfs.cpio.gz` boots here, with **none of the ~7 MB copy-window ceiling** that
  constrains the EA image.
- **The console is already serial**, at the same 115200 as every other board, and
  GRUB itself talks to it.
- **`timeout 0` + `hiddenmenu`** means no interactive menu appears. Selecting a
  different entry means **editing `default`**, not catching a prompt.
- **Adding a third entry doesn't disturb the first two.** The factory-restore
  path stays byte-identical, so recovery is untouched.

See [installing on the HC-800](/build/install/#hc-800) for the exact steps.

## Serial: five 8250 UARTs in the ISA range

```
Serial: 8250/16550 driver, 5 ports, IRQ sharing enabled
ttyS0  0x3f8  irq 4    16550A
ttyS1  0x2f8  irq 3    16550A
ttyS2  0x3e8  irq 11   16550A
ttyS3  0x2e8  irq 10   16550A
ttyS4  0x2f0  irq 5    16550A
```

:::caution[ttyS4 is the trap]
The vendor config sets `CONFIG_SERIAL_8250_NR_UARTS=5` **and**
`CONFIG_SERIAL_8250_RUNTIME_UARTS=5`. That second symbol is the one that bites:
mainline defaults `RUNTIME_UARTS` to 4, and `ttyS4` — the Zigbee NCP — simply does
not appear if it is left alone.
:::

| tty | I/O | Role | Evidence |
|---|---|---|---|
| `ttyS0` | 0x3f8 | serial console @115200 | `console [ttyS0] enabled` |
| `ttyS1` | 0x2f8 | **rear RS-232 port 1** | held by `ioserver` (fd 16) |
| `ttyS2` | 0x3e8 | **rear RS-232 port 2** | held by `ioserver` (fd 18) |
| `ttyS3` | 0x2e8 | **IO-MCU (LM3S1162)** @115200 | `/dev/ttySStellaris -> ttyS3`, held by `ioserver` (fd 14) |
| `ttyS4` | 0x2f0 | **Zigbee EM357 NCP** | `/dev/ttySZigbee -> ttyS4` |

**The two rear RS-232 ports are host UARTs from userspace's point of view**, not
MCU-routed like the EA family's combo ports. Anything talking serial on this board
can use a plain `/dev/ttyS*`, which the EA family cannot.

One caveat, because it is a real conflict rather than a detail: the
[IO-MCU page](/shared/io-mcu/) records that the LM3S1162 image configures
UART1/UART2 as "the two user ports". If that reading is right, then either those
MCU UARTs go somewhere other than the rear jacks, or the host 8250s are bridged
*through* the MCU rather than wired straight to the transceivers. **A running
system cannot tell those apart** — the fd map looks identical either way.

`ttyS4` was sitting at 9600 when read, but nothing had it open — that's the
untouched 8250 default, not a measurement of the NCP's rate.

## IO: an LM3S1162 on ttyS3

```xml
<Device type="hc800">
  <bootloader>IRBootloaderSerialLM3S1162.bin</bootloader>
  <application>700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin</application>
</Device>
```

`hc250` uses the **same two files**; the EA boards and `amp1` use the TM4C images.

The rear panel per the owner of this unit: **6 IR outputs, 4 relays, 4 contact
inputs**, plus the two RS-232 ports. All of that hangs off the LM3S1162. There are
no host GPIO lines, no `/sys/class` entries and no device nodes for any of it.

The relay and contact counts agree with the decoded TM4C profile table, which
predicts **4 + 4** for the nine-output blocks. **The IR count doesn't:** the
IO-MCU page reads the HC800 image as using TIMER0–3 and infers four IR channels.
Six jacks and four timers aren't incompatible — every Stellaris GPTM has two CCP
outputs, so TIMER0–3 can carry up to eight channels, but the four-channel figure
was inferred from timer count rather than from a pin table the way the EA numbers
were. **Neither number is firmware-confirmed for this board.**

The MCU's reset line is a host GPIO, so it can be held in reset or reflashed
without opening the case.

## GPIO: the ICH block, and the vendor's line names

`gpio_ich` (from `lpc_ich`) registers one chip: `base 206, ngpio 50`. Converting
the vendor's exports to **offsets within the chip**, which is what a modern kernel
and libgpiod want:

| Name | sysfs # | Offset | Dir | Value |
|---|---|---|---|---|
| `zigbee_reset` | 206 | **0** | out | 1 (released) |
| `wlan_disable` | 220 | **14** | out | 0 |
| `lan_disable` | 221 | **15** | out | 0 |
| `reset` | 230 | **24** | in | 0 — the ID/setup button |
| `io_reset` | 239 | **33** | out | 1 (MCU released) |
| `board_id0` | 240 | **34** | in | 0 |
| `board_id1` | 244 | **38** | in | 0 |
| `board_id2` | 245 | **39** | in | 1 |

**Mainline `gpio-ich` does not name lines**, so `gpiofind` won't work here the
way it does on the CA-1's device tree. Address these by chip label plus offset —
the sysfs numbers are an artefact of a dynamic base.

### LEDs — mapped by name, not yet by line

Six LED class devices exist, all from a `leds-gpio` platform device the vendor's
board file registers: `c4::power`, `c4::data`, `c4::network`, `wifi::red`,
`wifi::yellow`, `wifi::blue`. There's also a `gpio-keys-polled` device for the
button.

**Which ICH offsets these sit on is still unknown.** `CONFIG_DEBUG_FS` is off in
the vendor kernel so `/sys/kernel/debug/gpio` does not exist, and probing by
`export` is not conclusive: 30 of the 50 lines refuse to export, which mostly
reflects ICH pins not muxed as GPIO rather than lines a driver holds. Getting the
real map means disassembling the board file out of the vendor `bzImage`. Until
then openHC drives no LEDs on this board, everything else works without them.

## Network

**Wired:** `10ec:8168`, driven by Realtek's out-of-tree `r8168` in the vendor
kernel. Mainline's `r8169` covers this device; that's what openHC uses.

**Wi-Fi:** USB, not PCIe — `0bda:8172` on the internal EHCI root hub, an
**RTL8192SU**, driven by the in-tree staging driver:

```
r8712u: Staging version
usb 1-7: r8712u: Boot from EFUSE: Autoload OK
usb 1-7: r8712u: Loading firmware from "rtlwifi/rtl8712u.bin"
```

Two useful consequences: the driver is **already in mainline staging**, and its
firmware file is shipped by Buildroot's `linux-firmware` package, verified in the
2024.02.9 file list. No vendored blob and no post-build lift needed, unlike the
CA-1.

Note RTL8192SU is *not* covered by mainline's `rtl8xxxu`; staging `r8712u` is the
only driver. If it has been removed by 7.1.8, Wi-Fi is the thing that breaks, and
wired reachability is unaffected.

`wlan_disable` and `lan_disable` GPIOs can hard-disable either radio or NIC.

## Audio: ALC888-VD on ICH7 HD Audio

```
card 0: HDA Intel at 0xfe978000 irq 16
codec#2: Realtek ALC888-VD (0x10ec0888), subsystem 0x14a4d102
```

**The BIOS pin defaults describe the rear panel exactly**, so no model quirk is
needed — mainline `snd_hda_codec_realtek` will produce the right jacks:

| Node | Pin default | Jack |
|---|---|---|
| `0x14` | `0x01044110` | **Line Out**, ext rear |
| `0x1b` | `0x02244120` | **HP Out**, ext front — the second stereo output |
| `0x1a` | `0x01843150` | **Line In**, ext rear |
| `0x1e` | `0x01441140` | **S/PDIF Out**, ext rear — the coax digital out |
| `0x11` | `0x18561130` | Digital Out, *internal* HDMI |

That's the owner's "2 line out, 1 line in, 1 coax out", one for one. Every other
pin complex reads `0x411111f0` (not connected).

## Video: a real GPU, a fixed 720p pipe, and one missing piece

Two video chips are instantiated on the **SMBus** by the vendor's board code:

```
i2c i2c-6: new_device: Instantiated device ths8200 at 0x21
i2c i2c-6: new_device: Instantiated device adv7511 at 0x72
ths8200 6-0021: THS8200 Chip Detect SUCCESS!
  ##-- c4_adi_7513 --##  Chip Detect SUCCESS!
  ##-- c4_vid_conf --##  Intialized to 720p
```

A TI THS8200 video DAC and an ADI ADV7511 HDMI transmitter are fitted and
responding, and the vendor stack drives them to 720p at every boot. The hardware
matrix once called this board "none (headless)"; that is wrong at the silicon
level. (`i2c-0..i2c-5` are `i915 gmbus` buses; only `i2c-6` is the SMBus.)

### How the pipe is fed

`i915` is builtin in the vendor kernel and brings up `inteldrmfb` at
1280x720x32. The connector topology is the interesting part:

```
card0-LVDS-1   connected     edid: 0 bytes    modes: 1280x720
card0-VGA-1    disconnected
```

**Connected, with a zero-byte EDID and exactly one mode.** There is no display
negotiating anything — the timings come from the **BIOS VBT**, and the LVDS port
is wired to the THS8200/ADV7511 pair. Mainline i915 parses the same VBT, so a
modern kernel should light this pipe identically with no board code at all.

### The split that matters for a port

The two external chips are configured in different places, and only one of them
is free:

- **THS8200 is configured in-kernel, and it is GPL.** `ths8200.ko` is 11 KB and
  exports `ths8200_set_720P` / `_powerup` / `_powerdown`, driven by
  `c4_vid_conf.ko` (`Intialized to 720p`). Straightforward to redo from
  `/dev/i2c-6`.
- **ADV7511 is configured from userspace.** `c4_adi_hdmi.ko` is only an i2c
  chardev shim — `ioctl` read/write byte and block, major 250, device
  `c4_adi_7513`. The actual register writes live in
  **`/control4/lib/libvidcfg.so`**, used by `ioserver`. Mainline's
  `drm/bridge/adv7511` expects a device-tree bridge attachment and cannot bind
  to x86 i915, so **HDMI output needs a userspace i2c configurator**, and
  `libvidcfg.so` is the thing to reverse.

openHC now builds `CONFIG_DRM=y`, `CONFIG_DRM_I915=y`,
`CONFIG_DRM_FBDEV_EMULATION=y`, `CONFIG_FB=y` and `CONFIG_FB_DEVICE=y` so the
boot splash has a `/dev/fb0`. That lights the pipe and draws into it; **whether
a TV sees it is unproven** until the ADV7511 side is written.

:::caution[Two kconfig traps]
`DRM_FBDEV_EMULATION` selects `FB_CORE`, **not** `FB`, and `FB_DEVICE` — which
creates `/dev/fbX` — defaults to the value of `FB`. Set only the emulation
symbol and you get a working framebuffer console with no node for userspace to
open. And leave `FRAMEBUFFER_CONSOLE` **off**: fbcon clears the framebuffer when
it takes the surface, which is what ate the EA's first splash.
:::

### It costs almost nothing to use

Measured on the live unit, writing a full 1280x720x32 frame to `/dev/fb0`:

```
3686400 bytes (3.5MB) copied, 0.008101 seconds, 434.0 MB/s
```

**~8 ms of CPU to repaint the entire screen**, and ~13 ms/frame end to end from
a RAM source — a ceiling around 77 fps for pure blit. So a splash is free, a
status screen that repaints on change is unmeasurable, and only something
*animating* costs real CPU (~24% of one core at 30 fps, for the blit alone).
Getting pixels onto the panel is not the expensive part; rasterising them is.

### Can it run WPE WebKit?

Yes, and more easily than the EA family — with one large caveat.

Mesa 24.0.9's **`i915` gallium driver explicitly claims `0xa001 "Intel(R)
Pineview"`**, this exact chip. Because Mesa provides real EGL + GBM + Wayland,
this board needs **no custom libwpe backend**: the EA's `wpebackend-pvr` exists
only because the PowerVR DDK 1.7 predates Wayland. Stock `wpebackend-fdo` plus
**`cog` with `BR2_PACKAGE_COG_PLATFORM_DRM`** drives KMS directly through
GBM/EGL with no compositor.

Two prerequisites are config, not code:

- `BR2_PACKAGE_COG_PLATFORM_DRM` depends on `BR2_PACKAGE_HAS_UDEV` (libinput).
  Every openHC board uses busybox mdev, so this means switching to eudev.
- wpewebkit needs `BR2_TOOLCHAIN_BUILDROOT_CXX`, `BR2_INSTALL_LIBSTDCPP` and
  `BR2_USE_WCHAR`. Without them kconfig drops wpewebkit **silently** — the build
  succeeds and the engine is simply absent.

**The caveat is the fragment shader.** From `i915_screen.c` and `i915_reg.h`,
i915g offers GLSL **1.20** (GL 2.1 / GLES 2.0), **64 ALU + 32 TEX
instructions**, 4 texture indirections, **`MAX_CONTROL_FLOW_DEPTH = 0`** (every
`if` is flattened), **32 vec4 uniforms**, 10 varyings, one render target — and
**vertex shaders run on the CPU** via draw/gallivm. That is Shader-Model-2.0
class. Simple TextureMapper blits will compile; rounded-rect clips, blurs and
filters will not.

So build Mesa with **both** `GALLIUM_DRIVER_I915` and `GALLIUM_DRIVER_SWRAST`
and choose at runtime (`GALLIUM_DRIVER=i915` vs `llvmpipe`). llvmpipe is the
correctness fallback, but it wants `BR2_PACKAGE_MESA3D_LLVM` — a very long build
— and the D525 is **SSSE3-only, no SSE4.1/AVX**, 2c/4t at 1.8 GHz. Expect
single-digit to low-teens fps at 720p.

### What Control4 actually shipped

Worth knowing before assuming the GPU was ever exercised: **OS 3.x has no
graphics stack on this box at all** — no GL, no EGL, no Mesa, no X, no navigator
process. On-screen was an OS 2.10-and-earlier feature. The 2.10 GPL index
(`src-2.10.0.540110-res.xml`, 301 entries, and it covers `linux-3.16.38` — this
board's kernel) shows the UI stack was **DirectFB 1.4.2 / 1.6 plus GStreamer
1.10.3**. No Mesa, no X, no EGL anywhere in it. Control4 never used this GPU's
3D engine; the on-screen navigator was a 2D framebuffer UI.

(Mesa is MIT-licensed and so would not be *obliged* to appear in a GPL drop —
but DirectFB's presence is the positive evidence, not Mesa's absence.)

## Everything else that showed up

- **Watchdog:** `iTCO_wdt` (Intel TCO v1.11), and the vendor userspace actually
  runs `/sbin/watchdog -t 10 /dev/watchdog`.
- **RTC:** `rtc_cmos`, a normal PC CMOS RTC. Every other board in this tree needed
  an I²C RTC hunt; this one does not.
- **Thermal:** four ACPI cooling devices. No `hwmon` devices registered, the
  Atom's `coretemp` is simply not built in.
- **`flash-bios` in `/etc/init.d`** — the BIOS is field-flashable from Linux on
  this board. Not touched, not needed, and worth remembering as a way to brick a
  unit.
- **`/proc/config.gz` is present**, which made the vendor's kernel config directly
  readable rather than inferred. That's where `SERIAL_8250_RUNTIME_UARTS=5` came
  from.
- **Plain `scp` does not work** against this unit, and the failure isn't obvious:
  modern OpenSSH `scp` speaks SFTP, and the vendor's dropbear has no
  `sftp-server`, so it dies with `subsystem request failed on channel 0`. Use
  `scp -O` to force the legacy protocol, or pipe through `ssh`. Both were verified
  byte-for-byte. The SSH helper also races the dropbear pty handshake under
  `sshpass` roughly one connection in three and fails with "Permission denied";
  it works on retry, and a single `ControlMaster` session avoids it entirely.

## What this means for the port

The HC-800 is the **cheapest board in this repository to bring up**, ahead of even
the CA-1:

1. **Every driver it needs is mainline and has been for a decade** — `ahci`,
   `r8169`, `snd_hda_intel`, `i2c_i801`, `lpc_ich`/`gpio_ich`, `iTCO_wdt`, `8250`,
   `ehci`/`uhci`. The only staging component is the USB Wi-Fi. There's no SoC
   resurrection work, no de-device-tree patching, and no board DTS to reconstruct.
   **openHC needs zero kernel patches for this board.**
2. **The boot chain is a text file.**
3. **No image-size ceiling.** GRUB is not CEFDK.
4. **It's a PC, so it isn't a family.** Nothing about it generalises, which is
   why `board/hc800` is standalone with no shared base.

What is *not* solved by any of that's the Control4-specific IO: the LM3S1162 on
`ttyS3` still needs the [DLE/STX protocol](/shared/io-mcu/) implemented
before the IR outputs, relays and contacts do anything. That work is shared with
the EA family and with the HC-250.

## Open questions

- **The LED GPIO map**, six `leds-gpio` LEDs whose ICH offsets are only in the
  vendor board file.
- **The LM3S1162 profile table**, whether the image carries a per-board table
  like the decoded TM4C one. This would settle the six-jacks-versus-four-timers IR
  conflict and confirm the 4/4 relay/contact counts from firmware rather than from
  the panel.
- **Whether the two rear RS-232 jacks are wired to the host 8250s or bridged
  through the MCU's UART1/UART2.**
- **The Zigbee NCP's real baud rate**, never opened during this pull.
- **Whether the ADV7511/THS8200 video path terminates at a connector** on this
  revision.
- **BIOS boot-device options**, whether USB boot is available, which would give a
  second install path that touches the SSD not at all. Requires a serial console
  at power-on.
