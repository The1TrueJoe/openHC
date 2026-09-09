---
title: How IO is named
description: One naming convention across every openHC board — the kernel names it, userspace never carries a table of numbers.
sidebar:
  order: 1
---

Every openHC board answers the same questions with the same words. A relay is
`relay1` on a Control4 HC-800 and on an IO Extender, even though one reaches it
over a UART to a microcontroller and the other drives an SoC pin directly.

That is not tidiness. It is the difference between a box somebody can walk up to
and one that needs our documentation open beside it.

## The rule

> **The kernel names it. Userspace never carries a table of numbers.**

Every kernel *number* on these boards is an accident of probe order:

| number | what moves it |
|---|---|
| `/dev/lircN` | one minor counter shared by every rc driver in the kernel — a USB IR dongle shifts ours |
| `/dev/gpiochipN` | which chip registered first |
| `ttySN` | `CONFIG_SERIAL_8250_RUNTIME_UARTS`, whose mainline default silently drops `ttyS4` on an HC-800 |
| a GPIO offset | nothing — but it is board-specific, and a hand-kept list of them gets mis-sorted |

Names do not move. So a name is what everything above the kernel uses, and
converting a name to a number happens exactly once, in the kernel or in a udev
rule — never in a config file, a shell script, or a daemon.

## GPIO lines

Named where the kernel already knows them:

| board | mechanism |
|---|---|
| HC-800, EA1, EA3 | `gc->names` in `gpio-ohc-iomcu` — the lines are behind a microcontroller, so its driver is the only thing that can name them |
| CA-1, IO Extender | `gpio-line-names` on the controller node in the device tree |

Two shapes, one convention:

* **Indexed sets** are `<role><n>`, counting from **1**, matching the silkscreen:
  `relay1`, `contact4`. The gpiochip line *offset* stays zero-based, as an offset
  must — but nobody reads an offset off the back of a box.
* **Single-purpose lines** are `snake_case` roles: `zigbee_reset`,
  `setup_button`, `io_reset`, `led_link`.

Either way the client is the same everywhere:

```bash
gpioset $(gpiofind relay1)=1
gpioget $(gpiofind contact4)
```

`board.env` declares **how many**, never **which offset**.

## Device nodes

udev turns what the kernel reports into `/dev/ohc`, one directory per class:

```
/dev/ohc/ir/out1 … outN        rear IR jacks, as labelled
/dev/ohc/ir/front-blaster      internal emitter behind the front panel
/dev/ohc/ir/front-receiver     the learner on the same panel
/dev/ohc/serial/1 … N          user RS-232, in the order board.env declares
/dev/ohc/radio/zigbee          the NCP, wherever the SoC happens to wire it
/dev/ohc/radio/zwave
```

so the same command works on any board in the fleet:

```bash
ir-ctl -d /dev/ohc/ir/front-blaster --send=power.txt
```

**Relays, contacts and the gpiochip get no entry, and that is the point rather
than a gap.** They are lines, and lines are already named; a path to a chip
*number* would be a step backwards from `gpiofind relay1`.

**The IO microcontroller's own tty gets no entry either.** A line discipline
owns it, and its interface is the gpiochip and the lirc devices — pointing at
the raw port would only invite a second writer.

### Why the rules delegate

Both udev rules call `ohc-udev-name` rather than matching inline, because
neither answer is a property of the device itself. The IR label lives on the
*parent* rc device, not the lirc node. The serial numbering is `board.env`'s
declaration order. A match expression would be a second copy of each.

`OHC_SERIALS` and `OHC_RADIOS` therefore take the **same shape** — a bare kernel
device name, never a `/dev/` path, because constructing the path is our job:

```sh
OHC_SERIALS="ttyS1:RS-232_port_1:115200 ttyS2:RS-232_port_2:115200"
OHC_RADIOS="zigbee:ttyS4"
```

### Why eudev, not mdev

busybox mdev has no include mechanism, so shipping our rules would mean forking
Buildroot's `/etc/mdev.conf` and letting it drift on every bump. udev rules are
additive files. It also fires on hotplug rather than only at boot, which an init
script laying down symlinks cannot do.

## Where each board stands

| board | relays / contacts | IR | serial | radios |
|---|---|---|---|---|
| HC-800 | ✅ `relay1‑4`, `contact1‑4` | ✅ 6 jacks + blaster + receiver | ✅ `serial/1‑2` | ✅ `radio/zigbee` |
| EA3 | ✅ `relay1`, `contact1` | ✅ 6 + blaster + receiver | ⏳ MCU-routed, no tty yet | — |
| EA1 | — none fitted | ✅ 4 + blaster + receiver | ⏳ MCU-routed, no tty yet | — |
| CA-1 | — none fitted | — none fitted | ✅ `serial/1` | ✅ `radio/zigbee`, `radio/zwave` |
| IO Extender | ✅ `relay1‑8`, `contact1‑8` (device tree) | ⏳ FPGA block, no driver yet | ⏳ FPGA UARTs | — |

⏳ is hardware that exists and has no driver yet. When those land they inherit
the names above with no client change — that is what the convention buys.

## The one place it is not met

The HC-800's eight ICH lines — `zigbee_reset`, `io_reset`, `wlan_disable`,
`lan_disable`, `setup_button` and three board-revision straps — are still
offsets in `board.env`. Mainline's `gpio-ich` sets no line names, and being an
x86 PC there is no device-tree node to hang `gpio-line-names` on, so this is the
one board where neither mechanism above is available.

The honest fix is a small kernel patch attaching `gpio-line-names` as software
node properties to the `gpio_ich` platform device on a DMI match, in the same
spirit as the DaVinci patches the IO Extender already carries. Until that lands
it is the last number table in the tree, and it is called out here rather than
quietly tolerated.
