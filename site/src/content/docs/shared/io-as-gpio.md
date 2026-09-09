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
	line   0:      "relay1"       output
	line   1:      "relay2"       output
	line   2:      "relay3"       output
	line   3:      "relay4"       output
	line   4:    "contact1"        input
	line   5:    "contact2"        input
	line   6:    "contact3"        input
	line   7:    "contact4"        input

# gpioset $(gpiofind relay1)=1
# gpioget $(gpiofind contact1)
0
```

`iod` is now a **layer on top of this**, not a replacement for it. It still
provides the MQTT surface, the retained state, the settings and the config GUI —
but it reaches the hardware through `/dev/gpiochipN` like anything else would.

**There is no longer a fallback.** iod used to carry its own DLE/STX
implementation for boards whose kernel had no driver, and that path is gone: two
implementations of one wire format drift apart, and the second one is always the
one nobody tests. If the discipline is not attached, iod says so and serves
capabilities, and no relay or contact works — which is the truth, and better than
a daemon quietly fighting the kernel for the same UART.

## The whole chain

```
       relays, contacts                    the hardware
              │
      DLE/STX over a UART
              │
   gpio-ohc-iomcu  (line discipline)       the kernel owns the protocol
              │
      /dev/gpiochipN                       standard, and open to anything
        │          │
   gpioset       iod                       a client, like any other
                   │
                 MQTT                      the IO surface
                   │
        ┌──────────┴──────────┐
   config GUI          Home Assistant      both speak the same topics
```

Each layer is replaceable and none is privileged. `gpioset` and iod are peers.
The config GUI has no special access — it is an MQTT client, and its commands
are the ones an automation would send.

**iod does not hold the lines.** It requests, acts and releases for every
operation, exactly as `gpioset` does. A daemon that claimed `relay0..3` for its
lifetime would make every external tool fail with `EBUSY`, and the box would be
no more open than when a daemon owned the serial port.

## Numbering follows the panel

Line names, MQTT topics, the GUI and the serial WebSocket path all count from
**1**, because that is what is printed on the back of the box: `relay1` is the
terminal marked 1. An installer reading a silkscreen and an integrator writing
an automation see the same number.

Zero-based indices survive only where they are genuinely offsets — the gpiochip
line offset, and the selector on the wire to the microcontroller. Two places
convert, and only two: `mqtt::topics` in iod and the `panel()` helper in the GUI.

A topic numbered `0` is rejected rather than quietly treated as the first
device. Nothing is labelled 0, so a client sending it has almost certainly
assumed zero-based, and driving relay 1 for it would be the worst available
answer.

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

## What each board actually gets

| Board | Relays | Contacts | Lines on the chip |
|---|---|---|---|
| HC-800 | 4 | 4 | 8 |
| EA3 | 1 | 1 | 2 |
| EA1 | 0 | 0 | **none — the script skips** |
| CA-1 | — | — | no IO microcontroller at all |
| IO Extender V1 | 8 | 8 | native SoC GPIO; this driver is not involved |

The EA1 is worth calling out. It has an IO microcontroller, and it has no relays
or contacts — its IO is two MCU-routed UARTs and the IR jacks. The driver
refuses to register a chip with no lines, so `S12iomcu` checks the counts and
says so rather than letting a correct outcome look like a failure. The EA1's win
from moving this into the kernel is not GPIO; it is the serial work listed at the
bottom of this page.

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

## IR: one lirc device per emitter

The same driver registers the IR side through `rc-core`, so the jacks and the
front blaster are ordinary lirc devices and `ir-ctl` works without going through
iod at all.

**Every emitter gets its own node**, and each one is named:

```console
# for d in /sys/class/rc/rc*; do
>   printf '%s\t%s\n' "$(basename $d)" "$(sed -n 's/^DEV_NAME=//p' $d/uevent)"
> done
rc0	openHC IR out 1
rc1	openHC IR out 2
rc2	openHC IR out 3
rc3	openHC IR out 4
rc4	openHC IR out 5
rc5	openHC IR out 6
rc6	openHC IR front blaster
rc7	openHC IR front receiver

# ir-ctl -d /dev/lirc6 --send=power.txt      # out of the front blaster
# ir-ctl -d /dev/lirc7 --receive             # what the front receiver hears
```

:::note[`ir-ctl` is not in the image]
The nodes are standard, so the standard tools work — but `v4l-utils` needs a C++
toolchain the HC-800 image does not build, and forcing one on for a debug
utility is a poor trade. iod drives lirc directly with the same ioctls. Install
`v4l-utils` on any board whose toolchain has it, or use `ir/N/send` over MQTT.
:::

The alternative was one device plus `LIRC_SET_TRANSMITTER_MASK`, and it is worth
saying why that was rejected. The mask makes the output port a **mode** rather
than an address: a caller sets it, then transmits, and a second process that
sets the mask in between silently redirects the first one's code to the wrong
jack. Choosing the port by choosing the device removes the window entirely —
which the hardware allows, because the firmware carries the output mask in the
same frame as the durations. There is no port state on the part to preserve.

It also stops the receiver from advertising that it can transmit. Only the
emitters get `tx_ir`, so a client that opens the receiver and tries to send gets
an honest `ENOTTY` instead of radiating out of jack 1.

Names matter for the same reason line names do: `/dev/lirc3` is whatever probe
order made it, and on a box with a USB IR dongle plugged in it may not be ours
at all. The label in `DEV_NAME` is the stable handle, and it is how iod finds
each port — never by number.

:::note[The front panel is not "jack 7"]
The blaster is a different piece of hardware that happens to sit behind the same
opcode: an emitter pointed out of the case, with no socket to plug anything
into. Numbering it after the jacks would send somebody looking for a seventh
connector. It is named, and its MQTT topic is `ir/front/send` — sharing a prefix
with `ir/front/rx`, which is the receiver on the same panel.
:::

### The carrier survives a capture

A learned code is only replayable if you know what carrier it was learned at.
rc-core's raw events are durations in microseconds and carry no frequency, so
the driver emits a `LIRC_MODE2_FREQUENCY` event ahead of every capture with what
the firmware measured, then a `LIRC_MODE2_TIMEOUT` to mark the end of the code.
iod turns that back into a Pronto string — the same format `ir/front/send`
accepts, so a code learned on the front receiver can be sent straight back out
without any conversion in between.

## Still to come

- **MCU-routed serial as real ttys** on the EA family, the way `n_gsm` creates
  `/dev/gsmtty*`. Irrelevant on the HC-800, whose two RS-232 ports are host
  16550As, and genuinely valuable on EA, where those ports currently have no
  device node at all.
- **Contact interrupts**, so a `gpiomon` on a contact reports edges rather than
  the caller polling a cache that is already being polled.

## Two failures worth remembering

Both cost a build cycle each, and neither produced an error message anywhere.

**A request issued from the line discipline's `open()` can never be answered.**
`tty_set_ldisc()` holds `tty->ldisc_sem` for writing across the ldisc's open, and
the receive path needs that same lock to deliver bytes. So the reply cannot
arrive until `open()` returns. Identify is done from a work item now, once the
lock is gone.

**A line discipline that implements only `receive_buf` and never sets
`tty->receive_room` is handed nothing, forever.** The tty layer clamps delivery
to `receive_room`, which defaults to zero. Implement `receive_buf2` — it returns
what it consumed and is not clamped — or set `receive_room`, or the driver
transmits happily and observes silence.

The shared symptom is the nasty part: a microcontroller that answers a userspace
daemon perfectly and appears dead to the kernel driver on the same wire, at the
same baud, with the same termios. Everything points at the hardware, and the
hardware is fine.
