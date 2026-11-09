---
title: Device tree
description: How the EA board device trees are put together, the CE5300 PCI map they hang off, and the evidence behind each node.
sidebar:
  order: 14
---

Every EA kernel carries its board's device tree built in (`BUILTIN_DTB`, shared
patch 0015), picked by `CONFIG_BUILTIN_DTB_NAME`, which `build.sh` sets to the board name.
The shared sources are in `board/ea/common/kernel/arch/x86/boot/dts/`; each model
has its `<model>.dtsi` in `board/ea/<model>/` and each variant a short `.dts` in
`board/ea/<model>/<board>/`. The `.dtsi` files keep only one-line
comments; this page holds the background.

## Layout

| File | What it describes |
|---|---|
| `ce5300.dtsi` | The SoC: UARTs, the PCI functions board devices hang off, the boot flash. |
| `ea.dtsi` | What every EA board shares: framebuffer, LEDs, ID button, common GPIO hogs, rear USB power, heatsink sensor, fan and thermal zone, the IO-MCU. |
| `<model>.dtsi` (in `board/ea/<model>/`) | One per model: everything every variant of it has. Its IO-MCU jacks, relays and contacts, its switch ports, model-only lines, and the includes for the features the whole model has (`ea-switch`, `ea-audio-dsp`, `ea-audio-fpga`). |
| `<board>.dts` | One per variant: the model's `.dtsi`, `ea-wifi.dtsi` where there is a radio, ports for a variant-only switch, and `ea-gpio-names.dtsi` last. |
| `ea-wifi.dtsi`, `ea-switch.dtsi`, `ea-audio-dsp.dtsi`, `ea-audio-fpga.dtsi` | One per feature, with that feature's hogs. |
| `ea-gpio-names.dtsi` | `gpio-line-names` for all 128 lines. **Included last.** |

A board's `.dts` includes its model, then the feature files, in the same list as
its `ohc.features`, then `ea-gpio-names.dtsi`. A board without a feature never
gets that feature's nodes or drives its lines.

## How PCI devices get nodes

ACPI still owns the platform: CPUs, interrupts and PCI routing. The kernel's
`arch/x86/kernel/devicetree.c` only takes those from a tree when ACPI is
disabled. The tree exists to hang board devices off PCI functions.

A PCI device gets a node when one sits under its bus's node with `reg = <devfn>`.
`pcibios_get_phb_of_node()` finds the root bus by `device_type` + `bus-range`,
and `pci_set_of_node()` finds each device by the devfn in `reg[0]` (bits 15:8;
the bus bits above are informational). The PCI nodes have **no `compatible`** on
purpose: the PCI ids already bind the drivers, and a `compatible` on the root
node would make `of_platform_default_populate()` create a stray platform device.

The root's `ranges` (32-bit memory, identity-mapped at `0x80000000`) is
informational too. ACPI assigns the windows and nothing translates through these
nodes.

## The CE5300 PCI map

The layout follows the kernel's `arch/x86/platform/ce4100/falconfalls.dts`,
which the CE5300 keeps for the functions used here. Every devfn below was
measured with `lspci -tvnn` on an EA3; the A/V bridge sits at `00:01.0` as on
falconfalls.

| Function | ID | Driver | Node | Notes |
|---|---|---|---|---|
| 00:01.0 | 8086:0700 | — | `pci@1,0` | A/V bridge to bus 1 |
| 01:06.2 | 8086:2e60 | ce5300-i2s | `audio` | See [audio](/ea/audio/). |
| 01:0b.0 | 8086:2e66 | (none) | `serial@b,0` | UART1's PCI face. **Disabled**, see below. |
| 01:0b.1 | 8086:2e67 | gpio-intelce | `gpio` | 4 banks × 32 lines, base 0 |
| 01:0b.2 | 8086:2e68 | i2c-pxa-pci | `i2c@b,2` | Four adapters, one per BAR |
| 01:0b.4 | 8086:2e6a | spi-pxa2xx-pci | `ssp` | `CE5X00_SSP` via patch 0009 |
| 01:0b.6 | 8086:089f | pwm-ce5300 | `pwm` | The CE5300's own; not on falconfalls. Also measured on an EA1. |
| 01:0c.0 | 8086:2e6e | e1000 | `gbe` | `fixed-link`, patch 0003. The DSA conduit. |
| 01:0d.1 | 192e:0101 | ehci-pci | `usb_rear` | TDI host, patch 0014. The rear USB port. |
| 01:17.0 | 8086:08a0 | spi-ea-ce5xx | `bootflash` | Not on falconfalls; devfn from `lspci` on an EA1. |

`01:0d.0` and `01:0d.2` (the Zigbee CP2104) need no node.

**I²C.** `i2c-pxa-pci` makes one adapter per BAR, and its children are matched
by BAR index (patch 0001). The fourth adapter (BAR3, `0xdffe0e00`) carries the
audio DSP. The `i2c0`–`i2c3` aliases fix the adapter numbers, because
`i2c-pxa` takes them from aliases when it probes from DT, and both userspace
(the LM75 on `i2c-0`) and the audio machine driver (`adau1451.3-0038`) name buses
by number.

**SSP and boot flash.** For both SPI controllers the PCI device *is* the
controller, so their children are the SPI devices. The SSP carries the switch;
the boot flash is a separate, dedicated controller (see
[boot chain](/ea/boot-chain/#the-boot-spi-nor-is-now-writable-from-linux)).

**PWM.** The driver registers its chip against the PCI device, so the pwm core
finds it through the `pwm` node. It has no `of_xlate` of its own, so specifiers
are parsed by `of_pwm_xlate_with_flags`: `<channel period-ns flags>`. The
CE5300 PWM has four channels on one BAR.

## UARTs

The SoC's three GEN3 UARTs sit on legacy I/O ports. They are an XScale/PXA block
(`GEN3_serial` in Control4's kernel), clocked at 14.7456 MHz (why that matters
for the console: [boot chain](/ea/boot-chain/#three-board-quirks-you-will-hit)).

| Node | Port | Alias | Use |
|---|---|---|---|
| `uart0` | 0x3F8 | `serial0` | Console |
| `uart1` | 0x2F8 | `serial1` | The IO microcontroller, as a serdev device (no `ttyS1` of its own) |
| `uart3` | 0x2E8 | `serial3` | |

- **They run timer-polled.** Stock's routed IRQ (GSI 38) registers but never
  fires under openHC (measured), so the nodes deliberately have no `interrupts`.
  `8250_of` then gives the port irq 0 and serial8250 services it from a timer;
  TX polling is patch 0010.
- `compatible = "intel,xscale-uart"` fixes the port type, which replaced an old
  probe-override patch (0006). `clock-frequency` replaces the legacy table's
  1.8432 MHz for these ports.
- 8250 matches each node to the legacy slot at the same address, so `ttyS`
  numbers do not move. The aliases pin them too, matching the legacy slots.
- 0x3E8 is the 8051 power PIC, a real 8250. It is not in the tree and stays a
  plain legacy port.
- The parent node is named `isa` because that is what OF address translation
  keys on for I/O space (`of_bus_isa_match`). It is a `simple-bus` so the default
  populate creates the ports.

**UART1's PCI face stays disabled.** 01:0b.0 is the same block as the legacy
0x2F8 port the IO microcontroller is wired to. Binding it (`8250_pci`'s CE4100
entry) switches the block to PCI decode, which kills 0x2F8: TX counts go up and
the MCU never hears a byte. With `status = "disabled"`, `pci_bus_add_device()`
never lets a driver bind it. That replaced the old patch 0011.

## Boot flash

The chip under `bootflash` is the 16 MiB S25FL127S holding CEFDK and the Master
Flash Header, probed by JEDEC id. `spi-max-frequency` is 16.67 MHz, the clock
ratio the driver programs in `ce5xx_hw_init()`.

The partitions are named views of the regions CEFDK's mask ROM fixes, from
`mfh list spi_nor` on an EA1. The whole chip stays `mtd0`
(`MTD_PARTITIONED_MASTER`), so the offsets restore and the flasher write at keep
their meaning. The views are `mtd1`–`mtd3` and **read-only**, so nothing can
erase through one; writes go through `mtd0`.

| Partition | Offset | Contents |
|---|---|---|
| `cefdk` | 0x0 | `cefdk_s1`, `cefdk_s2`, `cefdk_s1h`, `cefdk_s2h`: the firmware itself |
| `mfh` | 0x80000 | The item table (SHA-256 at +0x1e0, checked by the mask ROM) and its chained banks, up to `uc8051_fw` at 0x80800 |
| `mfh-items` | 0x80800 | The items' contents: 8051 firmware, manifest, `ip_params`, `cefdk_params` (with the autoscript enable byte), splash, `plat_params`, openHC's script |

The items' offsets differ per box, so the MFH table is what locates them. See
[boot chain](/ea/boot-chain/) and [the EA-1 page](/ea/ea1/).

## Framebuffer

CEFDK leaves the VDC pipe running 720x480 out of a fixed buffer in an
e820-reserved window, and `simple-framebuffer` takes it over as `/dev/fb0`. The
values were read off the VDC on an EA-3: `upp_c` surface `0x7fc00000`, stride
`0xb80` (736 px × 4 = 2944 bytes). Size is stride × height. Why it stays
720x480: [graphics](/ea/graphics/#the-720x480-ceiling-and-why).

## LEDs and the ID button

The six `gpio-leds` were recovered verbatim from the stock kernel's `gpio_led[]`
(`num_leds = 6`); the two 4-ball LEDs are active low. The full table and how it
was extracted are on the [EA-3 page](/ea/ea3/#the-led-map-recovered-from-the-stock-kernel-binary).
The names are ABI: iod and `OHC_RESTORE_BUTTON_LED` address them under
`/sys/class/leds`.

The front ID button is GPIO 32, **active low** (released reads 1, pressed 0,
verified on an EA1). It triggers `ohc-restore-button`'s long-press
return-to-stock ([recovery](/shared/recovery/)).

- It is `gpio-keys-polled`, as on stock, because `gpio-intelce` has no
  interrupts. The 40 ms poll and 20 ms debounce come from the stock kernel's
  tables.
- The input device's name is ABI: udev links it as `/dev/ohc/input/id-button`
  (`70-openhc-io.rules`).
- Stock sent `KEY_F5`. openHC sends `KEY_RESTART`, the usual code for a reset
  button, which a browser on the display does not read as "reload".

## GPIO hogs and line names

The boot-time states come from stock `/etc/init.d/c4_gpio_config`. Outputs are
hogged high (released from reset, or enabled). A line no file hogs keeps
whatever CEFDK left it at.

**They have to be hogs, not an init script.** By the time userspace runs,
everything the kernel probes at boot has already missed its device. Measured on
an EA3: `gpio26 usb_2_serial_reset` was an output driven low, holding the
USB-serial bridge in reset.

**Where each hog lives.** `ea.dtsi` hogs only the lines every EA board uses
(`setup_gpio_common()`). A line that belongs to a feature (the switch, the DSP,
Wi-Fi) or one model (the EA-5's `usb_3_reset`) is hogged in that feature's or
model's `.dtsi`, so a board without the hardware never drives it.

| Line | GPIO | Hogged in | Notes |
|---|---|---|---|
| `io_reset` | 7 | `ea.dtsi` | IO microcontroller |
| `usb_2_serial_reset` | 26 | `ea.dtsi` | CP2104 in front of the EM357 Zigbee radio |
| `zigbee_reset` | 29 | `ea.dtsi` | EM357 released, so it runs its application from power-on |
| `ldo_enable` | 43 | `ea.dtsi` | |
| `n_fpga_reload` | 57 | `ea.dtsi` | |
| `n_twelve_volt_current_limit` | 75 | `ea.dtsi` | EA1/EA3 power block |
| `n_usb_current_limit` | 121 | `ea.dtsi` | EA1/EA3 power block |
| `gb_eth_reset`, `gb_sw_reset` | 5, 8 | `ea-switch.dtsi` | |
| `codec_reset`, `dsp_reset` | 24, 101 | `ea-adau1451.dtsi` | Boards with an ADAU1451 |
| `usb_3_reset` | 28 | `ea5.dtsi` | |
| `wlan_disable` | 27 | `ea-wifi.dtsi` | High = radio **enabled** |

**The power block.** Stock `setup_gpio_ea1()` and `setup_gpio_ea3()` both drive
75 and 121 high; `setup_gpio_ea3()` adds only `ac_pwr` (80, named in `ea3.dtsi`), and
`c4_board_name()` is "ea1" for the PoE board too. `poe_type` (54) and
`twelve_volt_ok` (122) are the inputs of the same block: named, not hogged.

**`wlan_disable`.** Stock drives it high in `setup_gpio_common()` and `wlan0`
comes up that way, so high is the radio's enabled state despite the name. See
also the [EA-1 page](/ea/ea1/).

**Names.** `gpio-line-names` is one property, so it cannot be split across files
the way the hogs are. `ea-gpio-names.dtsi` names the lines every board has, and
a feature or model line appears as `EA_LINE_<n>`, a macro the feature's or
model's `.dtsi` defines (next to its hog, if it has one). Unset, it falls back
to `""`. That is
why the file is included last, and why a named line means the hardware is there.
Names are stock `c4_gpio_config`'s where it has one; the few that are openHC's
(`usb_rear_power`, `usb_rear_oc`) say so.

Inputs (board-id straps, status and fault flags) are named but never hogged, so
`gpioget`/`gpiofind` can read them. GPIO 32 (`id_button`) is the exception: the
`gpio-keys-polled` node holds it. GPIOs 17–23 are the seven straps the stock
kernel's `id_straps()` reads (revision = bits 0–3, board type = bits 4–6), so
23 is `board_id6`; the stock script's `src_reset` on 23 is commented out. How
names reach userspace: [IO naming](/shared/io-naming/).

## Rear USB power

The rear USB port (EHCI 01:0d.1) is powered through a load switch on GPIO 44.
That line is **not** in the stock script. Stock's closed `c4overcurrent` driver
drives it, registered by `ninjago_board_init()` on every board type with platform
data `{ gpio_oc = 97, gpio_pwr = 44 }`. Its probe drives 44 low, waits, then
high. On a falling edge of 97 (the load switch's active-low fault flag) it cuts
44 for 2500 jiffies and powers it back up.

Measured on an EA3 (PR #51): the bootloader leaves 44 low and the port sees
nothing (PORTSC SE0, no connect). Driven high, a USB stick enumerates as
high-speed; low again, it disconnects.

openHC describes 44 as a `regulator-fixed` that is the controller's
`vbus-supply` (patch 0016), not a hog. It follows root-hub port power, so
usbcore's port control cuts the port's 5 V:

```
echo 1 > /sys/bus/usb/devices/<roothub>-0:1.0/<roothub>-port1/disable
```

or `uhubctl`. That needs the regulator core, which only the fat kernel carries
(`linux/real-kernel.fragment`). The size-limited bootstrap kernel has none, so
on it the rear port stays unpowered; it only has to reach p1 and kexec.

The names `usb_rear_power` and `usb_rear_oc` are openHC's; the vendor's are only
the driver's `gpio_pwr`/`gpio_oc`. Nothing acts on the fault line yet. It is
named so it can be watched.

## Fan and thermal zone

The fan is a mainline `pwm-fan` on `pwm` channel 2, the cooling device of the
`heatsink-thermal` zone, which reads the LM75 at `i2c-0` 0x48. The 100 kHz PWM,
the 30% floor, the trip curve, manual control and the 95 °C critical trip are
all on the [fan page](/ea/fan/).

This replaced the shell daemon `ohc-fand`. The curve is `ohc-fand`'s (the stock
`[ea_fan]` profile with a 30% floor), and sysmond's 80 °C hot trip, which ends a
manual hold, was `ohc-fand`'s CRITICAL. A device tree cannot say "hold for
60 s", so the first trip's 1 °C hysteresis stands in for `ohc-fand`'s cool-down
hold at the floor. The LM75 and fan `label`s ("Heatsink", "Fan") are the hwmon
labels the Health panel shows.

## The GbE fixed link

The GbE MAC has no PHY on any EA: nothing answers on its MDIO bus, and the
MAC talks RGMII straight to a switch port (the BCM53125's port 5 on the EA3). The `gbe` node says so the
standard way:

```dts
gbe: ethernet@c,0 {
	reg = <0x16000 0 0 0 0>;
	phy-mode = "rgmii";

	fixed-link {
		speed = <1000>;
		full-duplex;
	};
};
```

Mainline `e1000` ignores all of this and fails probe with `-EIO` after
scanning 32 MDIO addresses. Patch `0003` makes it read the node instead:

- **Probe** takes speed and duplex from `fixed-link` and skips the PHY scan.
  Only `phy-mode = "rgmii"` is accepted. No `pause` in the node means flow
  control off, which matches the switch side (port 5's override, 0x4b, has
  no pause bits).
- **Link setup** forces the MAC: CTRL gets `SLU|FRCSPD|FRCDPX` with the speed
  bits and `FD` written explicitly, not inherited from CEFDK, and CTL_AUX bit
  0 is cleared (RGMII).
- **Link state** is always up, with no PHY reads.
- **PHY access** fails: register reads and writes return an error, PHY reset
  is a no-op, the MII ioctls, `ethtool -s` and the PHY loopback self-test
  return `EOPNOTSUPP` or are skipped. `ethtool eth0` reports 1000/full on an
  MII port.

The node is read through the device's OF node, not its fwnode. 01:0c.0 has
no ACPI companion (the DSDT's GbE `_ADR` is the wildcard `0x000CFFFF`, which
Linux never matches; no `firmware_node` in sysfs), so today the two are the
same. If patch 0017 is ever replaced by keeping the bridge's ACPI companion
as the fwnode, `dev->of_node` still points at this node and e1000 keeps
working.

It replaced Control4's "internal fake PHY" mode (ported from their GPL drop),
which answered MDIO reads from a canned register table with a made-up
BCM53125 PHY ID and keyed on the CE5300 host bridge's PCI ID. That one also
set CTL_AUX bit 1; see below for what that bit turned out to be.

The patch is meant for net-next. A DT binding question comes with it: the
node's compatible would be the generic `pci8086,2e6e`, which has no binding
today (falconfalls' CE4100 tree already uses it undocumented).

## The switch

`ea-switch.dtsi` puts the BCM53125 on SSP chip-select 1 with its CPU port, for
the EA3s, the EA-5s and the `ea1-v2-poe`. Each of those declares its own user
ports under `&switch_ports`: `lan1` and `lan2` on the EA3 and ea1-v2-poe, `lan0`
to `lan4` on the EA-5. The port map and the evidence for it are on the
[switch page](/ea/switch/). What the node itself rests on, all measured on a
live EA3:

- **SPI mode 3 at 1843200 Hz**, from Control4's own
  `b53SpiInit()`/`spiMasterConfig()`: SPH=1, SPO=1, `clock_div` 1 = the SSP's
  3686400 / 2. In mode 0 transfers complete but the switch never answers. That
  cost the most time.
- **Port 5 is the CPU port, not the IMP port 8.** Link summary 0x24 (ports 2
  and 5), their MIB counters mirror each other, and port 5's GMII override reads
  0x4b (forced 1000/full), the other end of the GbE fixed link. Port 2 is the
  primary jack; port 1 is the second jack, isolated in the stock config.
- **The CPU port has no `phy-mode`/`fixed-link` on purpose.** b53 is on DSA's
  `dsa_switches_apply_workarounds` list, so DSA logs that and skips phylink for
  the port, leaving firmware's forced link alone, as the old platform data did.
  Add a `fixed-link` only after proving b53's port-5 reconfiguration keeps the
  link.
- **The node is always present.** Whether b53 binds is `S39ea-switch`'s call: it
  loads `b53_spi` only with `ohc.switch=1`, because a half-attached switch takes
  the SoC MAC with it.

The EA-5's extra ports (0, 3, 4) are added in `ea5.dts`; which jack is which is
unverified.

## IO microcontroller

The IO microcontroller (TI Tiva TM4C1231D5) is a serdev child of `uart1` at
460800 baud, bound by `gpio-ohc-iomcu`. Each model's `.dtsi` lists its jacks,
relays and contacts. The protocol and per-board details are on the
[IO-MCU page](/shared/io-mcu/).
