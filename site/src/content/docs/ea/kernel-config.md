---
title: Kernel configuration
description: Why the EA kernel fragments set what they set — the command line and RAM map, the copy-window size budget, the fat real kernel, the SoC drivers and each feature's kernel half.
sidebar:
  order: 15
---

The EA kernel is the mainline i386 defconfig plus a stack of fragments, merged
in this order:

| Fragment | Holds |
|---|---|
| `board/ea/common/linux/common.fragment` | the family delta: serial, USB, SoC drivers, the size trims |
| `board/ea/<board>/linux/<board>.fragment` | the command line and the board's built-in device tree |
| `board/ea/common/features/<feature>/linux.fragment` | each feature's kernel half, appended by `build.sh` after the board file |
| `board/ea/common/linux/real-kernel.fragment` | the fat real kernel only, merged last |

Later fragments win, which is why anything `common.fragment` turns off (DRM,
framebuffer, sound, wireless, MMC, ext4) can only come back from a feature.

Buildroot's `merge_config.sh` scrapes anything that looks like an assignment,
comments included. A symbol written with `=` or followed by "is not set" in a
comment shows up in the merge diagnostics as a redefinition, so the fragments
name symbols without them.

Some symbols cannot be set from a fragment at all, and kconfig drops the line
without a word. Each of these has bitten the build once:

| Line that does nothing | Why | What works |
|---|---|---|
| `CONFIG_NET_DSA_B53*` | the symbols do not exist | `CONFIG_B53`, `CONFIG_B53_SPI_DRIVER` |
| `CONFIG_PWM_CE5300` | `pwm-ce5300.c` is copied in as `obj-y`, no Kconfig entry | nothing; `CONFIG_PWM` is enough |
| `CONFIG_SPI_PXA2XX_PCI` | derived: `def_tristate SPI_PXA2XX && PCI && COMMON_CLK` | `CONFIG_COMMON_CLK` + `CONFIG_SPI_PXA2XX` |
| `CONFIG_MTD_SPI_NOR` with SPI off | depends on SPI | `CONFIG_SPI` first |
| `CONFIG_I2C_PXA` on x86_64 | depends on `X86_32` upstream | patch `0013` widens it to `X86` |
| unsetting `DRM_KMS_HELPER`, `DRM_DISPLAY_HELPER` | no prompt, selected | see [graphics](/ea/graphics/#what-it-costs) |
| unsetting `SYSTEM_DATA_VERIFICATION`, the trusted keyring | selected by other options | nothing; see [below](#module-signing) |

## Command line and RAM map

`CONFIG_CMDLINE` is per board; `common.fragment` only turns on
`CMDLINE_BOOL` and `CMDLINE_EXTEND`. With `CMDLINE_EXTEND` the bootloader's line
is appended after the built-in one, so a later `pci=` on the `bootlinux` line
wins.

### `pci=realloc,nocrs`

Required on every CE5300 board, tested both ways on an EA3:

- `realloc`: CEFDK leaves the ath9k and e1000 BARs unassigned
  ([boot chain](/ea/boot-chain/#three-board-quirks-you-will-hit)).
- `nocrs`: the ACPI `_CRS` declares only a 48 KB TPM window for the root bus and
  never mentions `0xc0000000-0xdfffffff`, where every on-SoC device lives.
  Without `nocrs` the bus-01 bridge window "can't claim" and the kernel hangs at
  about 1.02 s, before userspace. With it the kernel derives windows from E820
  and boots. On the EA1 this pair is what brings up `wlan0`.

### `pci=routeirq`

On every EA line, to route the CE5300 UART interrupt link (ACPI `LUAR`, GSI 38)
for the IO-MCU on `ttyS1`. On stock that interrupt fires; under openHC it
registers but counted 0 across a full transmit, so the ports run timer-polled
([serial](#serial-ports)). Patch `0010` drives TX off the LSR, because the GEN3
block does not reflect THRE in the IIR.

### RAM reclaim (`memmap=exactmap`)

CEFDK's e820 gives Linux only `[0x00100000-0x0c7fffff]` (200 MB) and marks
`[0x0c800000-0x7fffffff]` (1.8 GB) "device reserved": the Intel CE media
carve-out, real idle DRAM that openHC never loads
([EA1 RAM](/ea/ea1/#ram-cefdk-withholds-most-of-it)). Reclaiming it is what lets
WPE WebKit run; it needs more than 1 GB.

It has to be `memmap=exactmap`. The additive form is silently dropped: e820
sanitisation resolves overlaps toward the higher type, so "reserved" beats an
added "RAM". Every entry is load-bearing:

| Entry | What it is |
|---|---|
| `68K@0` | low memory, reproduced as CEFDK gives it |
| `60K#0x11000` | the ACPI tables. Claiming them as RAM gives `AE_NO_ACPI_TABLES`; the box boots to 706 MB and hangs |
| `512K@0x20000` | rest of low memory |
| `255M@1M` | CEFDK's 200 MB region merged with the gap above it, to 256 MB |
| `1536M@0x10000000` | the reclaim, ending at `0x70000000` |

That leaves 128 MB of margin below a hard stop at `0x78000000`. Do not widen
past it:

- the framebuffer at `0x7fc00000` must not become System RAM, or simplefb scans
  out of memory the kernel hands out. The DTB framebuffer has no System-RAM
  guard, unlike the old `ce5300-fb`;
- the region just under it holds CEFDK structures the box needs to reboot.

The map crosses the 32-bit lowmem boundary (about 934 MB of highmem); userspace
is unaffected. The figures describe the EA3 v2's CEFDK split. Another split
needs its own numbers, and a `memmap` past the end of real DRAM is not
recoverable remotely.

| Board | Where the reclaim lives | Status |
|---|---|---|
| ea3-v2 | `CONFIG_CMDLINE` | proven 2026-08-31: `MemTotal` 190688 → 1807608 kB, ACPI clean, boots to userspace |
| ea3-v1 | `CONFIG_CMDLINE`, same line | proven: a v1 EA3 has run the ea3-v2 image with this line durably (1.7 GB) |
| ea5 | `CONFIG_CMDLINE`, same line | copied from the EA3, unverified on an EA-5 |
| ea1-v1, ea1-v2, ea1-v2-poe | `OHC_KEXEC_CMDLINE_EXTRA` in `board.env` | proven on an EA1: `MemTotal` ~176 MB → ~1.7 GB |

The EA1 boards boot through the `S00kexec` bootstrap, whose `kexec
--command-line` carries the real kernel's line; `S00kexec` appends
`OHC_KEXEC_CMDLINE_EXTRA` to it. Without the reclaim an EA3 v1 boots with
CEFDK's ~200 MB, too little to unpack the 166 MB RAM rootfs (`Initramfs
unpacking failed: write error`, seen on a netboot).

## The size budget

CEFDK's `bootlinux` copies the kernel into a fixed window of **7,417,856
bytes**; a bigger kernel overwrites the loader mid-copy and the box fails to
boot ([the copy window](/ea/boot-chain/#the-copy-window) has the mechanism and
the measurements). Recovery then costs the factory-restore button. The stock
i386 defconfig comes out at about 8.3 MB, hence `KERNEL_XZ`,
`CC_OPTIMIZE_FOR_SIZE` and the trims.

**New functionality goes in as a module, not built in.** Modules are the only
real headroom: the large subsystems live on p1 instead of in the bzImage, and
udev loads them from their devices' modaliases. A GPU driver as a module is
also `rmmod`/`insmod`-iterable instead of a reflash.

What must stay built in:

- whatever reaches the rootfs: e1000, sdhci/mmc, ext4, 8250 serial, PCI, ACPI;
- the SoC drivers the board DTB's devices hang off: `gpio-intelce` (LEDs, reset
  hogs) and `i2c-pxa` (LM75).

With modules off, every `=m` symbol inherited from the defconfig is silently
dropped. Turning modules on resurrects the whole lot at once (`i915` among them,
on a board whose GPU is a PowerVR SGX545), so module families nobody wants are
disabled explicitly.

What individual pieces cost against the window:

| Piece | Cost | Where it went |
|---|---|---|
| DRM core + helpers | ~1.25 MiB | module (`sgx`), built in on the real kernel |
| cfg80211 + mac80211 + ath9k | ~1 MB | modules (`wifi`); an `emmc`-only kernel was already 7,037,952 |
| storage stack | put ea1-v2 at 8.3 MB | modules (`storage`) |
| NFS v3/v4 client + root-on-NFS, 9P, SUNRPC, FAT built in under VFAT | ~450 KB, when ea1-v2 had 76.8 KB spare | disabled in `storage` |
| DSA + b53 + tag codecs | pushed the EA3 over | modules (`switch`) |
| bridge | ~60 KB | module (`switch`) |
| simplefb | ~150 KB | built in (`display`) |
| Buildroot 2026.02 toolchain | ~100 KB larger kernel; ea3-v1 and ea1-v2-poe ran over | `BRIDGE`, `VLAN_8021Q` moved to modules |

### The trims

Each is safe on this hardware:

- **NICs**: the box has exactly one network interface, the CE5300's e1000
  behind the BCM53125. `E1000E`, `SKY2`, `8139TOO` and `R8169` came from the
  generic defconfig and never had anything to bind to. `ISO9660_FS` goes too.
- **EFI**: boot is CEFDK's legacy 16-bit `bootlinux` entry, never EFI.
- **Paravirt/guest, debug, trace, BPF, KGDB**: bare metal, no instrumentation.
- **`KALLSYMS`**: a big slice of a small kernel. Turn it back on to resolve a
  crash.
- **Networking**: plain IPv4 for SSH and HTTP. No netfilter, IPsec, IPv6 or QoS.
- **Watchdog, power-supply class**: not used. The thermal framework stays on; it
  runs the fan.

Two are off for reasons other than size:

- **Stack protector.** The Buildroot musl toolchain's SSP canary and the 7.1.8
  per-CPU canary setup disagree. The secondary CPU's guard does not match, so
  `__stack_chk_fail` panics as soon as `swapper/1` enters `do_idle`
  (`stack-protector: Kernel stack is corrupted in: do_idle`).
- **`WERROR`.** The mainline x86 defconfigs set it. Buildroot's `linux.mk` forced
  it off through 2025.x; 2026.02 dropped that. This kernel builds the vendor SGX
  DDK (32-bit-era code, hundreds of size and format warnings on x86_64), and a
  newer gcc must not fail the image over a warning.

## The fat real kernel

openHC EA boot is two kernels:

| Kernel | Lives | Built from | Job |
|---|---|---|---|
| bootstrap | the CEFDK slot | common + board + features | stay inside the copy window, reach p1 or the network, kexec the real kernel |
| real | `/boot/bzImage` on p1 | the same, plus `real-kernel.fragment` | run the system |

`S00kexec` (`board/ea/common/rootfs-overlay/etc/init.d/`) does the hand-off.
kexec has no size limit, so `real-kernel.fragment` builds the large, always-present
subsystems in and the size check in `post-image.sh` is never in the way. Modules
stay on; the ASoC audio stack is still `obj-m` and loads from p1.

- **GPU.** `CONFIG_DRM` built in brings the DRM core and KMS/display helpers
  (~1.25 MiB), the only reason it was a module. `SGX545_CE` depends on DRM and
  follows it; its `objs.mk` line is `obj-$(CONFIG_SGX545_CE)`. With the GPU built
  in it is simply present at boot, which retired the `pvrsrvkm` rmmod/insmod
  dance.
- **simplefb** stays built in, as on the bootstrap.
- **Regulator core**, for the rear USB port's 5 V switch (GPIO 44), which the
  board DTB makes the EHCI's `vbus-supply` (patch `0016`). It does not fit the
  bootstrap, where the rear port stays unpowered
  ([rear USB power](/ea/device-tree/#rear-usb-power)).

### Keep the IOMMU off

Building DRM in pulls `IOMMU_SUPPORT`, and on x86 `INTEL_IOMMU` then defaults on.
The fat kernel came up with VT-d DMA remapping active, which the lean kernel
never had. Cedarview's VT-d is buggy: the SDHCI controller's ADMA transfers hit
an empty translation and the eMMC never initialises. On a live EA1:

```
mmc0: sdhci: ADMA Err: 0x00000001
mmc0: Timeout waiting for hardware interrupt
error -110 whilst initialising MMC card
```

With `rootwait` the box then hangs before mounting p1. The real kernel has no
IOMMU driver at all, and `S00kexec` also passes `intel_iommu=off`.

### Module signing

Off: secure boot is off on this hardware and the bootstrap signs nothing, so
there is no key and nothing to enforce. The trusted keyring and
`SYSTEM_DATA_VERIFICATION` are selected by other options and cannot be unset.
They pull in `certs/extract-cert`, a host tool; `mk-real-kernel.sh` builds it
against the runner's own openssl (`HOSTPKG_CONFIG`) so it links.

## Serial ports

The SoC UARTs come from the device tree through `8250_of` (`SERIAL_OF_PLATFORM`),
timer-polled, each in the legacy-table slot at its address; the [device tree
page](/ea/device-tree/#uarts) has the nodes, why they are polled, and why
UART1's PCI face stays disabled. Four slots are enough because the device-tree
ports reuse the legacy ones.

`DETECT_IRQ` is off. The i386 defconfig sets it, which adds `UPF_AUTO_IRQ` and
runs the ISA IRQ auto-probe on the legacy ports. Nothing here wants an IRQ
reassigned behind its back.

## USB host

Three EHCI controllers, PCI `192e:0101`, a TransDimension (TDI) core. The
USB-serial side (`cp210x`) for the Zigbee NCP is in the shared `zigbee` feature.

- Patch `0014` sets `ehci->has_hostpc` for this core. Without it every port dies
  at probe with `can't setup: -110` (reset handshake timeout).
- The same quirk sets `hcd->has_tt`: the TT is embedded and there is no companion
  OHCI/UHCI. So `USB_EHCI_ROOT_HUB_TT` is required for the full-speed CP2104 to
  enumerate; without it a full-speed device sits at `CCS=0`.

## Wired network

The on-board Intel GbE (`e1000`) is what CEFDK netboots from. On the EA1 it is a
second path beside Wi-Fi; on boards without a radio it is the only one. The MAC
has no PHY on the MDIO bus, so the PHY autodetect found nothing and
`e1000_probe()` returned `-EIO`; patch `0003` ports Control4's fake-PHY mode
(proven on hardware 2026-08-26, `eth0` link up). The [switch page](/ea/switch/)
has the details, and why the second jack needs DSA.

## SoC drivers

### Device tree

`bootlinux` passes no DTB, so the board's own is linked into the kernel (patch
`board/common/patches/linux/0015`, `BUILTIN_DTB`) and picked per board by
`CONFIG_BUILTIN_DTB_NAME`. The trees replaced the old C board files; layout and
nodes are on the [device tree page](/ea/device-tree/). `kexec --dtb` overrides
the built-in tree, so a test tree needs no rebuild.

This is not the CE4100 platform: `X86_INTEL_CE` is 32-bit only and replaces
ACPI. The EA keeps ACPI and adds a device tree beside it.

### GPIO, LEDs, ID button

The GPIO controller is PCI `8086:2e67`. Mainline's `gpio-sodaville` binds it
with the CE4100 layout, 12 lines; the CE5300 has 4 banks of 32, 128 lines, for
the reset and LED pins. So sodaville is off and openHC's `gpio-intelce` binds
`8086:2e67` and exposes all 128. Three front-panel LEDs (`gpio-leds` in
`ea.dtsi`) sit on GPIOs 99–102, beyond what sodaville exposed.

The front ID button is `gpio-keys-polled` in `ea.dtsi`, read by
`ohc-restore-button`. It is a module; udev loads it off the node's modalias.

Userspace GPIO is libgpiod 1.6.4 (Buildroot 2024.02.9), which speaks the v1
chardev ABI. 7.1.8 gates that behind `GPIO_CDEV_V1` (deprecated, default off);
without it `gpioset`, `gpioget` and `gpioinfo` fail with `EINVAL` on a working
chip (measured on an EA3). `GPIO_SYSFS_LEGACY` keeps `/sys/class/gpio/export` as
a fallback.

### I²C, clocks, sensors, fan

`i2c-pxa` (the `REGS_CE4100` variant) sits behind PCI `8086:2e68` and needs two
patches on this x86_64 kernel:

- `0001` enumerates the CE5300's fourth controller, binds each adapter to its
  node in the board DTB, and makes i2c-pxa's clock optional;
- `0013` widens `I2C_PXA`/`I2C_PXA_PCI` from `X86_32` to `X86`. Without it the
  symbol is dropped and there is no I²C bus at all: no LM75 or fan, no DS1339
  RTC, no codec.

`COMMON_CLK` must be on for SPI (see the table at the top). The SoC registers no
clock provider, which is why `0001` makes the clock optional: with `COMMON_CLK`
a required clock would fail every adapter with `-ENOENT`.

The LM75 heatsink sensor is on `i2c-0` at `0x48`
(`/sys/class/hwmon/hwmon0/temp1_input`). A DS1339 RTC is also on `i2c-0`, at
`0x68`, a later `hwclock` candidate.

The fan PWM driver, `pwm-ce5300.c` (PCI `8086:089f`), is copied in as plain
`obj-y`. The fan itself is `pwm-fan` in a thermal zone on the LM75, so the
kernel runs the curve in both kernels and the fan is never without a
controller. `pwm-fan` treats the supply as optional, so the bootstrap, which has
no regulator core, runs it too. `step_wise` runs the curve; `user_space` is the
policy sysmond switches to while the web UI holds the fan. The curve and the
PWM are on the [fan page](/ea/fan/).

### Boot SPI-NOR

The boot autoscript lives in SPI-NOR (`mtd0` at `0x91000`) and hardcodes byte
counts for the kernel and initrd, smaller than any x86_64 image openHC builds.
Exposing the flash lets it be rewritten to read the whole slot, leaving
`bootlinux`'s window as the only limit. The controller, partitions and what the
install writes are on the [boot chain page](/ea/boot-chain/#the-boot-spi-nor-is-now-writable-from-linux).
Back up `mtd0` before writing to it: a bad MFH or autoscript costs the button
and the CEFDK shell to recover.

The boot flash is not on the pxa2xx SSP (`8086:2e6a`) the switch rides. That was
assumed once and proven wrong on an EA1 (`SPI transfer failed: -5`).

`SPI` comes first because `MTD_SPI_NOR` depends on it. One build had `MTD` and
`MTD_BLOCK` but no `MTD_SPI_NOR` because SPI was inherited off.
`MTD_OF_PARTS` reads the DTB's read-only partitions
([boot flash](/ea/device-tree/#boot-flash)); `MTD_PARTITIONED_MASTER` keeps the
whole chip as `mtd0` beside them. Restore, boot-init and the flasher all write
at whole-chip offsets into `/dev/mtd0`; without it `mtd0` would become `cefdk`.

## Features

Each feature's kernel half is merged after the board fragment. Unless noted,
everything is a module for the copy window.

### display

simplefb on the framebuffer CEFDK leaves running (~150 KB, built in). No fbcon:
the splash is drawn straight onto `/dev/fb0` and the console stays on `ttyS0`;
a text console clears the framebuffer on takeover and ate the first splash.

### sgx

Userspace finds the GPU through libdrm's `drmOpen()`, so a DRM node must exist
([graphics](/ea/graphics/#the-correction-drm-is-not-optional)). The driver
registers no modesetting and owns no GEM objects; display stays on simplefb.
`SGX545_CE` is the driver's own Kconfig symbol (sgx545ce, mirrored in-tree via
`objs.mk`), not a Buildroot package. DRM and `SGX545_CE` are modules on the bootstrap (the driver depends on DRM, so
module is the only option while DRM is one) and built in on the real kernel.

- `X86_PAT` and `MTRR` give real write-combining for the SGX command buffers.
  Without them the driver falls back to uncached: correct, markedly slower.
- `DEVMEM` without `STRICT_DEVMEM`: `sgxregs` reads the SGX identity registers
  through `/dev/mem` before any driver is loaded, and strict devmem blocks
  exactly that BAR.
- DRM drivers for hardware the board lacks (`i915` alone is 2.6 MB, plus
  virtio-gpu, TTM, `INTEL_GTT`, `AGP`) are off. They cost nothing in the bzImage
  either way; this keeps them off the rootfs.

### audio-hdmi, audio-dsp, adau1451, audio-fpga

The [audio page](/ea/audio/) covers the hardware.

- **audio-hdmi** is in `ea/common/ohc.features`, since every EA has HDMI. The
  driver, `sound/x86/ce5300-hdmi.c`, is copied in through `objs.mk` with no
  Kconfig symbol: it builds whenever `SND` is set, because `sound/Makefile`
  descends into `sound/x86` under `SND`. udev loads it off PCI `8086:2e63`.
  `SND_PCM` has no prompt and nothing else selects it on a board without ASoC;
  the loopback card (`SND_ALOOP`) does, and is useful in its own right as a
  capture of what is playing.
- **adau1451** is shared by the EA-3 (`audio-dsp`) and EA-5 (`audio-fpga`).
  `adau1451-c4` is copied in, so it has no Kconfig entry to select `REGMAP_I2C`;
  the fragment sets it. The sigmadsp core has no prompt; the ADAU1701 driver is
  the supported way to get it built.
- **audio-dsp** (EA-3): CE5300 I2S to the ADAU1451 as a `simple-audio-card`.
- **audio-fpga** (EA-5): the Spartan-6 audio FPGA on PCIe needs `PCI_MSI`.

### emmc

The SD controller is PCI `8086:070b` (class `0x0805`). Mainline `sdhci-pci` binds
it through its `PCI_DEVICE_CLASS(SYSTEM_SDHCI)` entry; no out-of-tree driver is
needed. MMC and ext4 are built in: p1 is the root filesystem.

### restore

The MFH lives in the boot SPI-NOR. MTD is already on in `common.fragment`; the
fragment only asserts the `/dev/mtd0` interface `restore` opens, since a feature
ships both halves.

### storage

The shared `board/common/features/storage` fragment, as modules. eudev loads USB
disk drivers from the modalias, `mount(2)` loads a filesystem by its
`fs-<type>` alias, and storaged modprobes `ksmbd` before starting
`ksmbd.mountd`. ext4 stays built in for `emmc`.

Turning on `NETWORK_FILESYSTEMS` (for ksmbd) brings back the defconfig's NFS
v3/v4 client with root-on-NFS and 9P, SUNRPC and its crypto, built in; and
`MSDOS_FS` built in would force FAT in under the VFAT module. None of it is used,
so it is disabled (see the cost table).

### switch

On ea3-v1, ea3-v2 and ea1-v2-poe. Three pieces: the SPI master `spi-pxa2xx-pci`
on PCI `8086:2e6a`, the b53 driver, and the switch node in `ea-switch.dtsi`. The
[switch page](/ea/switch/) has the port map and hardware results.

- `COMMON_CLK` is required. Without it the SPI master never binds `8086:2e6a`
  and b53 never probes (measured: `01:0b.4` with no driver, empty
  `/sys/class/spi_master`).
- The symbols are `B53` and `B53_SPI_DRIVER`. For a long time this fragment set
  `NET_DSA_B53*`, which do not exist; b53 was never built (`spi92.1` appeared
  with modalias `spi:bcm53125`, `/sys/bus/spi/drivers/` was empty). `B53` selects
  the BRCM tags.
- DSA, b53 and the tag codecs are modules. That is safe because the device
  comes from the DTB, so nothing built in calls a b53 or DSA symbol; the module
  binds the existing SPI device when it loads. `S39ea-switch` loads it, only with
  `ohc.switch=1`, before `S40net`, so DSA still attaches at boot. The SPI master
  stays built in. `BRIDGE` and `VLAN_8021Q` are modules too, loaded by the same
  script (DSA wants bridging present).
- `BRIDGE_VLAN_FILTERING` is a bool compiled into `bridge.ko`, so it costs the
  bzImage nothing. Without it `ip link set <br> type bridge vlan_filtering 1`
  fails with `Operation not supported` (measured on an EA3), and switchd's
  VLAN programming (`program_vlans`, `bridge vlan add`) can never engage.

### usb-input

The fleet `board/common/features/usb-input` fragment with the controller
drivers as modules; eudev loads each on plug-in. Keyboards and mice (evdev,
usbhid, hid-generic) are already in the common kernel.

### wifi

Atheros AR9485 on PCIe, mainline ath9k, no firmware blob; on ea1-v1, ea1-v2 and
ea3-v1. `common.fragment` turns the wireless stack off so radio-less boards do
not carry it, and this fragment turns it back on.

Modules are not optional: built in, cfg80211, mac80211 and ath9k (~1 MB) give a
kernel that does not boot, which looks like dead hardware. The radio comes up
from `S41wifi-ap` after p1 mounts. `CFG80211` and `MAC80211` must be modules too:
a built-in cannot depend on a module, so building them in would drag ath9k back
in with them.

The WPA crypto (AES, CCM, GCM, ARC4) stays built in on purpose: it is small,
partly pulled in elsewhere, and a missing algorithm shows up as an unexplained
association failure rather than a load error.
