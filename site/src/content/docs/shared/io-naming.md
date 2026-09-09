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
/dev/ohc/gpiochip              the chip carrying the relays and contacts
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

### Why relays and contacts have no node of their own

This is the question everyone asks, and the answer is not a design preference:
**Linux has no per-line device node.** A GPIO line is addressed by *offset
inside a chip*, through an ioctl on the chip's chardev. There is no kernel
object for a single line to hang a node on.

So `/dev/ohc/relay1` could only ever be a symlink to the whole gpiochip — and
`/dev/ohc/relay2` would point at the same file. Opening it would not give you
relay 1. That is a lie dressed as a convenience, and the moment somebody wrote
`echo 1 > /dev/ohc/relay1` expecting it to work, we would have made the box
*harder* to walk up to, not easier.

The other tempting answer is a per-line sysfs attribute, like the old
`/sys/class/gpio/gpioN/value`. That interface is deprecated and being removed
precisely because it has no ownership model and no atomicity — reviving it in a
new driver would be going backwards.

What a line has instead is a **name**, which is strictly better than a path
would have been: it is unique fleet-wide, it survives renumbering, and it is
what the modern tools already speak.

```console
# gpioinfo /dev/ohc/gpiochip
gpiochip1 - 8 lines:
	line   0:  "relay1"    output
	...
	line   7:  "contact4"  input

# gpioset $(gpiofind relay1)=1
```

The **chip** does get an entry, because a chip *is* a device and its number is
exactly the sort of thing that moves.

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

## Where it is not met yet: the two x86 boards' own lines

The relays, contacts and IR above are done. The boards' **other** GPIO — resets,
enables, straps, the setup button — is not, on either x86 family:

| board | lines | state |
|---|---|---|
| HC-800 | `zigbee_reset`, `io_reset`, `wlan_disable`, `lan_disable`, `setup_button`, 3 straps | unnamed; offsets in `board.env` |
| EA1 / EA3 | `dsp_reset`, `codec_reset`, `io_reset`, `zigbee_reset`, `usb_2_serial_reset`, … | named as **consumer labels**, not line names |

The EA case is the subtler of the two and worth being exact about, because it
looks done and is not. `gpio-ea-board.c` calls
`gpio_request_one(101, flags, "dsp_reset")`, and a request sets the line's
**consumer**, not its **name**. Those are different fields in
`GPIO_V2_GET_LINEINFO_IOCTL`:

```console
# gpioinfo | grep 101
	line 101:  unnamed  "dsp_reset"  output
	           ^^^^^^^  ^^^^^^^^^^^
	           name     consumer

# gpiofind dsp_reset      # finds nothing — gpiofind matches on NAME
```

So they show up in `/sys/kernel/debug/gpio` and are invisible to every tool that
resolves by name. Different fields, and only one of them is the handle.

### The fix, and why it is one fix and not two

Neither board has a device tree, so neither can carry `gpio-line-names` the way
the CA-1 and the IO Extender do. But gpiolib does not actually require a device
tree — it reads that property off the **parent device** of the gpiochip, through
the generic `device_property_*` API. Device tree is only one of the things that
can back it. On x86 the equivalent is a **software node**, which is precisely
what it exists for.

So both boards get the same property, attached to the parent before the GPIO
driver binds:

| board | parent of the gpiochip | where to attach |
|---|---|---|
| EA1 / EA3 | PCI `8086:2e67` | `DECLARE_PCI_FIXUP_EARLY` in `gpio-ea-board.c` |
| HC-800 | the `gpio_ich` platform device from `lpc_ich` | a platform-bus notifier on `BUS_NOTIFY_ADD_DEVICE` |

After which `gpiofind dsp_reset` and `gpiofind setup_button` work on every board
in the fleet, and `OHC_GPIO_*` leaves `board.env` the way `OHC_RELAY_GPIOS` did.

### Why not the vendor's `/dev/gpio/dsp_reset`

Control4's own OS exposes exactly the per-line nodes this page says Linux does
not have — `echo 1 > /dev/gpio/dsp_reset` works on a stock EA3. It is worth
saying why openHC does not copy that.

Their kernel is 3.16 and predates the GPIO character device, which landed in
4.8. With no chardev ABI to use, a bespoke driver exporting a node per line was
a reasonable thing to write. It is not reasonable now: it would mean shipping a
parallel implementation of gpiolib that no standard tool speaks — not libgpiod,
not Home Assistant's GPIO integration, not anything a user already has. The
whole argument for putting this IO in the kernel was that stock tools should
work on it, and a private `/dev/gpio` would undo that.

The name survives; only the mechanism changes. `dsp_reset` is still `dsp_reset`
— it is just a line name now, reachable with the tools everyone has.
