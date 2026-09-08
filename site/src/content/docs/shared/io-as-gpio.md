---
title: IO as GPIO
description: The IO microcontroller behind a standard Linux gpiochip — why a line discipline rather than serdev, how to attach it, and what userspace sees.
sidebar:
  order: 2
---

The relays and contacts on a Control4 controller do not hang off the SoC. They
hang off a companion microcontroller reached over a host UART, speaking the
[DLE/STX protocol](/shared/io-mcu/).

The obvious way to use that is a daemon that owns the port and offers an API.
openHC started there, and it works — but it makes every consumer a client of
*our* daemon. Nothing else on the box can close a relay. `gpioset` cannot.
Home Assistant's stock GPIO integration cannot. Every integration has to be
written against openHC specifically.

So the protocol moved into the kernel. `gpio-ohc-iomcu` presents the relays and
contacts as a **standard `gpiochip`**:

```console
# gpiodetect
gpiochip0 [gpio_ich] (50 lines)
gpiochip1 [ohc-iomcu] (8 lines)

# gpioinfo ohc-iomcu
gpiochip1 - 8 lines:
	line   0:      "relay0"       output
	line   1:      "relay1"       output
	line   2:      "relay2"       output
	line   3:      "relay3"       output
	line   4:    "contact0"        input
	line   5:    "contact1"        input
	line   6:    "contact2"        input
	line   7:    "contact3"        input

# gpioset $(gpiofind relay0)=1
# gpioget $(gpiofind contact0)
0
```

`iod` is now a **layer on top of this**, not a replacement for it. It still
provides the MQTT surface, the retained state, the settings and the config GUI —
but it reaches the hardware through `/dev/gpiochipN` like anything else would.

## Why a line discipline, not serdev

`serdev` is the modern way a kernel driver claims a UART instead of leaving it
to userspace, and it is the wrong mechanism here.

serdev devices are **enumerated by firmware** — a device tree node or an ACPI
device that says "there is a thing on this port". The HC-800 and the EA family
are x86 PCs whose BIOS describes 8250 UARTs and nothing else. No firmware on
earth is going to declare a Control4 IO co-processor, so no serdev device is
created and there is nothing for a driver to bind to.

A **line discipline** does not need enumeration. Userspace opens the port and
hands it to the kernel with `TIOCSETD`; from that moment the driver owns the
byte stream. It is how PPP, SLIP and `n_gsm` are attached, and it puts the
choice of *which* port in userspace — where the board description already lives
— instead of hardcoding a tty in a driver.

:::note[The number is `N_DEVELOPMENT`]
The discipline registers as 29, the slot the kernel reserves for out-of-tree
line disciplines. It is overridable with the `ldisc_num` parameter, because the
numbering is a uapi constant that has grown over time and a collision should be
a loud module-load failure an operator can work around — not a reason to patch
a header.
:::

## Attaching it

`/etc/init.d/S12iomcu` does this at boot, from `board.env`:

```sh
echo 4 > /sys/module/gpio_ohc_iomcu/parameters/relays
echo 4 > /sys/module/gpio_ohc_iomcu/parameters/contacts
ldattach -s 115200 -8 -n -1 29 /dev/ttyS3
```

It runs **early** — before `iod` and `webd` — because those are clients of the
chip. `ldattach` stays running: the discipline lives as long as the fd is open.

The geometry is set through sysfs rather than passed to `modprobe` because these
images are deliberately all-builtin (`CONFIG_MODULES=n` — self-contained image,
no depmod, no load ordering, no `.ko` in the initrd). A built-in's parameters
would otherwise only be settable on the kernel command line. The driver latches
the counts when the discipline attaches, so a later sysfs write affects the next
attach and never a chip that has already told userspace what its lines are.

| board.env key | what it does |
|---|---|
| `OHC_IO_BACKEND` | `mcu` — anything else and the script does nothing |
| `OHC_IO_MCU_TTY` | which port the part is on (`/dev/ttyS3` on HC-800, `/dev/ttyS1` on EA) |
| `OHC_IO_MCU_BAUD` | 115200 on HC-800, **460800** on EA — see the [protocol page](/shared/io-mcu/) |
| `OHC_RELAYS` / `OHC_CONTACTS` | how many lines to expose |
| `OHC_IO_MCU_LDISC` | discipline number, default 29 |

## Two details that are not obvious

**There is no SET opcode.** The firmware offers only `RELAY_GET` and
`RELAY_TOGGLE`, so the driver reads first and toggles only on a mismatch. That
is not tidiness: `gpiod_set_value()` is expected to be idempotent, and a
toggle-only implementation would invert a relay every time something wrote the
value it already had — including a retained MQTT message replayed on a broker
reconnect.

**Contacts have no notification.** The protocol has no unsolicited push for
them; the host polls. The driver does that once, on a work queue, and serves
reads from the cache. Doing it in the kernel is the point: five clients asking
"is the door open" become five cached reads instead of five round trips down a
UART that answers one question at a time.

## The chip refuses to appear for a dead microcontroller

The driver identifies the part before registering anything. If the MCU does not
answer, `ldattach` succeeds and **no gpiochip is created**.

That is deliberate. A gpiochip standing in for a microcontroller that is not
talking is a chip whose every read is a lie, and it would be indistinguishable
from working hardware right up until somebody trusted a contact.

:::caution[A wedged microcontroller is a real failure mode]
A malformed IR payload has been observed to stop the part answering *anything* —
relays, contacts, identify — and it does not recover on its own, through a
detach and re-attach, or through a reboot. Pulsing the documented reset GPIO did
not revive it either, at either polarity. The recovery is a power cycle. See the
[IO microcontroller page](/shared/io-mcu/) for what triggered it.
:::

## Still to come

- **IR through `rc-core`**, giving `/dev/lirc0` and making `ir-ctl` and
  `ir-keytable` work. Bigger than it sounds: rc-core speaks raw timing and the
  MCU speaks Pronto-style bursts, so there is a real conversion layer.
- **MCU-routed serial as real ttys** on the EA family, the way `n_gsm` creates
  `/dev/gsmtty*`. Irrelevant on the HC-800, whose two RS-232 ports are host
  16550As, and genuinely valuable on EA, where those ports currently have no
  device node at all.
- **Contact interrupts**, so a `gpiomon` on a contact reports edges rather than
  the caller polling a cache that is already being polled.
