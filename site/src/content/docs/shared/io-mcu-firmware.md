---
title: IO-MCU firmware
description: openHC's own firmware for the IO microcontroller, how the driver configures and loads it, the device-tree binding, and returning the part to stock.
sidebar:
  order: 4
---

openHC carries its own firmware for the [IO microcontroller](/shared/io-mcu/): one
image per part, configured at run time by the kernel driver from the board's
device description. It speaks Control4's wire protocol, so nothing above the driver
changes, and the driver works the same against it and against stock.

| Part | Boards | Image |
|---|---|---|
| TM4C1231D5 | EA-1, EA-3, EA-5 | `/lib/firmware/openhc/io-mcu-tm4c1231d5.bin` |
| LM3S1162 | HC-800 | `/lib/firmware/openhc/io-mcu-lm3s1162.bin` |

:::caution[Run on one EA-3 so far]
The TM4C image has run on an EA-3 (2026-11-04): the host link, the pin table,
IR out, the relay (heard) and contact, a driver rebind, and the writer in both
directions. The rest of the EA IO is accepted on it from the decode. The LM3S
image has not run on a part. No board sets
`firmware-name`, so no box writes either on its own. See
[what is left](#what-hardware-verification-is-left).
:::

## How it fits together

```
board DT / ACPI _DSD ──> gpio-ohc-iomcu ──CONFIG_SET──> firmware applies the pins
                              │
       firmware-name ─────────┤ request_firmware(), compare versions,
                              │ write over the TI bootloader on a mismatch
                              │
  /sys/class/firmware/ohc-iomcu <── restore writes Control4's image back
```

1. The driver binds, knocks and starts the application (the
   [bring-up](/shared/io-mcu/#bring-up-sequence) is unchanged), reads
   `FIRMWARE_VERSION`, and sends **one config frame** built from the node.
2. With `firmware-name` on the node, it loads that image and **writes it only when
   the part reports a different version**.
3. Return to stock writes Control4's own image back through the same writer.

## The firmware

The source is `board/common/firmware/io-mcu/`, beside the driver it talks to:

```
core/          hardware-free: protocol codec, config validation, IR engine, the app
parts/         each part's pin functions: the data the config frame is checked against
hal/<part>/    registers, vector table, linker script
test/          host tests against a fake HAL, replaying frames captured off live units
tools/mkfw.py  the image header, wrapping a stock image, the version
```

Constants shared with the driver are in
`board/common/kernel/drivers/gpio/ohc-iomcu-proto.h`. Buildroot builds both images
with `host-arm-gnu-toolchain` whenever a board has the `iomcu` feature; CI runs the
host tests (`make test`).

**Nothing is driven until the host says so.** At start the firmware touches the
host UART and nothing else. There are no board profiles and no ADC strap: the
stock image's per-board tables and its board-ID strap are not used at all.

| Part | IR carrier | Receiver | Bursts | Clock |
|---|---|---|---|---|
| TM4C1231D5 | timer CCP in PWM mode (no PWM module) | 32-bit wide-timer edge-time capture | TIMER4, 32-bit one-shot | 80 MHz, PIOSC + PLL |
| LM3S1162 | PWM module (generator *n* drives outputs 2*n*, 2*n*+1) | GPIO edge interrupt, timestamped by TIMER1 | TIMER0, 32-bit one-shot | 50 MHz, 6 MHz crystal + PLL |

**Nothing is inherited from the bootloader.** Both stock bootloaders' `RUN` gate
off UART0 and reset it (legacy `RCGC1` bit 0, `SRCR1`) before they jump, so the
application finds its host link unclocked: the first read of it bus-faults. That
was the hang on the first run on silicon. The firmware therefore sets the clock
the way stock and its bootloader do — `SysCtlClockSet(0xC1000550)` on the TM4C
(PIOSC → PLL → 80 MHz), `RCC = 0x01C002C0` on the LM3S (6 MHz crystal → PLL →
50 MHz, which is why stock answers `AUTO_BAUD` with 115207) — and clocks, muxes
and programs UART0 from scratch. The PLL lock is read from `PLLSTAT` on the TM4C,
not the lock interrupt, which an already-locked PLL never raises again.

**IR out** is interrupt driven: the burst timer's interrupt gates the carrier onto
the pin (the pin is handed between the timer and a GPIO held low), so the main
loop's latency never reaches the waveform. The reply goes out first (status 0, as
stock), then an unsolicited `IROUT_STATUS` with 4 when the code is done, or 1 if
`IROUT_STOP` ended it. A second code while one is going out gets status 3 (busy).
A carrier word of 0, which hard-faults stock, is refused. The carrier duty is a
third.

**IR in** reduces carrier-rate edges to bursts as they arrive (a 100 ms code is
thousands of edges, too many to store), measures the carrier, and reports
`IRIN_CAPTURED` in stock's format: the period in 50 MHz ticks whatever the part's
clock, then marks and spaces in carrier periods, chunked at 100 words. A receiver
that outputs the envelope instead of the carrier works too (`control4,demodulated`).
Capture pauses while transmitting.

**User serial** (`UART_SET_CONTROL`/`SEND`/`RECEIVE`) is interrupt driven both ways.
A port's pins stay inputs until it is switched on, so a combo jack's IR output and
serial port never both drive it.

**A crash resets into the bootloader**, which never starts the application by
itself, so a faulted part is found in its bootloader at the next probe rather than
spinning silently.

### The config frame

`CONFIG_SET` (`0xf0`) carries `[version 1, count, count × {kind, index, port, pin,
flags}]`. The reply (`0xf1`) is `[version, result, count, one status per entry]`.

| Kind | Index | Pin must be |
|---|---|---|
| 1 IR output | `IROUT_SEND` bit | its own carrier: a timer CCP (TM4C) or PWM output (LM3S) |
| 2 IR receiver | 0 | TM4C: a 32-bit timer CCP; LM3S: any pin |
| 3 relay | `RELAY_GET` index | any pin |
| 4 contact | `CONTACT_GET` bit | any pin |
| 5 / 6 serial RX / TX | `UART_*` port | RX and TX of one UART |
| 7 IR carrier | source 0..3 | a shared carrier: CCP or PWM, as kind 1 |
| 8 IR gate | `IROUT_SEND` bit | any pin: routes carrier *n* (flags) onto that output |
| 9 hold | 0.. | any pin, held at a fixed level |
| 10 heartbeat | 0 | any pin, toggled every 100 ms |
| 11 / 12 serial mode A / B | `UART_*` port | any pin: a combo jack's IR/serial lines |

Flags: `0x01` active-low (relay, contact, gate, envelope receiver), `0x02`/`0x04`
pull up / down (contact), `0x08` the receiver outputs the envelope, `0x10` hold
high, bits 5–6 a gate's carrier.

The two IR shapes are the two boards'. On the EA every jack is a timer CCP of its
own (kind 1). On the HC-800 there are two PWM carriers (`PD1`, `PF3`, kind 7) and
each jack has a pair of active-low lines that route one of them onto it (kind 8),
plus a third line stock only ever drives low (kind 9); see the
[pin-map decode](/shared/io-mcu-pinmap/#lm3s1162-config-block). For a code, the
firmware opens each selected jack's gate to the lowest-numbered configured carrier,
gates that carrier per burst, and closes the gates after.

A combo jack's two mode lines follow `UART_SET_CONTROL`: enable 1 drives A high
(serial), enable 0 both low (IR). `UART_MODE` (`0xa3`, `[port, 1]`) drives B instead:
serial with RX and TX swapped, stock's null-modem option.

Each entry is checked against the part's own pin functions, from the TM4C123x
datasheet and StellarisWare's `pin_map.h` for the LM3S1162, and the ones it cannot
do are refused one by one: unknown kind, index out of range or used twice, no such
pin, a reserved pin (the host link, JTAG), a pin without the function, a pin or
timer half or UART already taken, a serial RX/TX that is not one UART's pair, bad
flags. Everything else is applied. The driver logs each refusal by name. The same
table sent again (a driver rebind) changes nothing, so a closed relay stays closed.

**Stock ignores it.** Neither stock dispatcher handles `0xf0` or `0xf2`: the LM3S
image compares down a chain that ends `… 0xa1, 0xb9, 0xc1, 0xc4, 0xd2`, the TM4C one
`… 0xa1, 0xa3, 0xc1, 0xc4, 0xd2`, and anything else falls through to nothing. The
driver doesn't rely on that: it reads the firmware version first and sends the
table only to openHC's own.

`RESET` (`0xf2`, answered `0xf3`) resets openHC's firmware into the bootloader,
which the loader uses to replace it without a reset line.

## The binding

The node is `control4,<part>-io-mcu` plus `control4,io-mcu`, a child of the host
UART. The binding is
`board/common/kernel/Documentation/devicetree/bindings/gpio/control4,io-mcu.yaml`;
pins are `<port pin>` with the ports from `dt-bindings/gpio/control4,io-mcu.h`.

```dts
#include <dt-bindings/gpio/control4,io-mcu.h>

&uart1 {
	io_mcu: io-mcu {
		compatible = "control4,tm4c1231d5-io-mcu", "control4,io-mcu";
		current-speed = <460800>;
		firmware-name = "openhc/io-mcu-tm4c1231d5.bin";	/* opt in */
		reset-gpios = <&gpio 7 GPIO_ACTIVE_LOW>;
		control4,heartbeat-pin = <IOMCU_PORT_C 7>;
		#address-cells = <1>;
		#size-cells = <0>;

		jack@1 {
			reg = <1>;				/* panel number */
			label = "IR / Serial 1";
			control4,ir-channel = <0>;		/* IROUT_SEND bit */
			control4,ir-pin = <IOMCU_PORT_B 6>;
			control4,uart = <0>;			/* ttyOHC0 */
			control4,uart-pins = <IOMCU_PORT_E 4>, <IOMCU_PORT_E 5>;	/* RX, TX */
			control4,uart-mux-pins = <IOMCU_PORT_E 2>, <IOMCU_PORT_B 1>;	/* A, B */
		};
		ir-blaster {
			control4,ir-channel = <6>;
			control4,ir-pin = <IOMCU_PORT_D 4>;
		};
		ir-receiver {
			control4,ir-pin = <IOMCU_PORT_D 6>;
		};
		relays {
			#address-cells = <1>;
			#size-cells = <0>;
			relay@0 {
				reg = <0>;
				control4,pin = <IOMCU_PORT_A 2>;
			};
		};
		contacts {
			#address-cells = <1>;
			#size-cells = <0>;
			contact@0 {
				reg = <0>;
				control4,pin = <IOMCU_PORT_F 0>;	/* closed = low */
				control4,active-low;
				bias-pull-up;
			};
		};
	};
};
```

These are the EA-3's pins from the [pin-map decode](/shared/io-mcu-pinmap/); the
board files get theirs once each is checked on hardware. A gated board (the HC-800)
says `control4,ir-carriers = <IOMCU_PORT_D 1>, <IOMCU_PORT_F 3>;`,
`control4,ir-gates-active-low;` and, per jack, `control4,ir-gates = <port pin
carrier>, …;` instead of `control4,ir-pin`, with its third lines in
`control4,hold-low-pins`. A node without pins still works
against stock firmware, and the old count properties (`control4,relays`,
`control4,ir-outputs`, the `control4,ir-blaster` boolean, …) are still read for
nodes that have no children of that kind.

The HC-800 gives the same shape in ACPI: `board/hc800/acpi/ssdt-iomcu.asl` has
`jack@1..6`, `ir-blaster`, `relays` and `contacts` as `_DSD` hierarchical data
nodes, with no pins yet.

## The loader

Opt-in per board: without `firmware-name` nothing is ever written to the part.

1. Built into the kernel, the driver may probe before the root filesystem is
   mounted; it retries `request_firmware()` for up to three minutes.
2. It checks the image header (magic, CRCs, the part from the compatible, a load
   address of exactly `0x1000`) and compares its version with what the part
   reported. Equal: nothing happens.
3. It gets the part into its bootloader: openHC's firmware resets itself on `RESET`;
   anything else needs `reset-gpios`. Stock with no reset line is left running, with
   a warning.
4. It writes through the TI serial bootloader (`DOWNLOAD`, `SEND_DATA` in 32-byte
   packets, `GET_STATUS` after each), confirmed against both stock bootloaders:
   `DOWNLOAD` erases page by page before it ACKs, it accepts only `0x1000` (or
   `0x0`, its own region) as the start, and `RUN` always starts the application
   through the vector table at `0x1000`.
5. It starts the application, reads its version back, and sends the config frame.

**An interrupted write recovers itself.** The whole slot is written once with a
small stub in place of its first page (the stub's reset vector writes
`SYSRESETREQ`), then a second `DOWNLOAD` at `0x1000`, one page long, erases that
page and writes the real one. Cut short anywhere before that last page, starting the application
lands straight back in the bootloader; the next probe finds the bootloader
answering where the application should be, and writes again. A write that fails
three times leaves the part in its bootloader with nothing sent to it.

**The bootloader is never written.** Every address is at or above `0x1000`.

The image is a 64-byte header (`OHCIOMCU`, part, load address, size, CRC-32,
the version string `FIRMWARE_VERSION` answers with, a header CRC-32) followed by
the bytes for flash. The version is `openhc-<VERSION>+<hash of the sources>`, so
any change to the firmware is a new version.

`/sys/class/ohc/iomcu/firmware` and `…/part` show what runs on which part.

## Return to stock

`restore stock` puts Control4's own application back, from Control4's own files:
`/control4/firmware/io/` in the stock rootfs, picking the image `.flash.config`
names for the board (`ea1`/`ea3`/`ea5` the TM4C 1.0.36 image, `hc800` the LM3S
03.26.15 container). It wraps it in the header above (checking the LM3S
container's CRC-16) and writes it through the driver's firmware-upload device,
`/sys/class/firmware/ohc-iomcu`, then checks the part answers with stock's version.
A part already on stock is left alone.

- **EA:** last in the finalize boot-init runs, from p1 just unpacked from
  `recfs.tar.xz` — the openHC kernel there may itself have written openHC's image at
  probe. A failure there is logged, not fatal: p1 is already stock.
- **HC-800:** first, before the GRUB one-shot is armed, from the recovery archive
  (`OHC_STOCK_ARCHIVE`, or `OHC_ZIGBEE_STOCK_ARCHIVE`). On failure nothing else
  has changed.
- `restore iomcu` runs only this step.

The flasher's restore runs the box's own `restore stock`, so it gets the same.

## What hardware verification is left

Done on an EA-3 (TM4C, 2026-11-04): the image starts after `RUN` and answers;
the pin table applies (23 of 23); NEC from jack 1 reads back on a GC-IRL learner
as 38 kHz with a 9.0 / 4.5 ms leader and the right bits; relay 1 is heard
switching on each toggle and reads back, the contact reads; a driver rebind
finds it by its knock and leaves the relay closed; the writer goes stock → openHC over the reset line and
openHC → stock over `RESET`, repeatedly. One write's first attempt got no ACK
after the reset pulse and the second worked; the retry covers it. Writing
stock's full 60 KB slot takes about 90 s (one `GET_STATUS` per 32-byte packet).

Accepted on the EA-3 without being exercised one by one, since the same image
and pin table carry them: IR duty, channels gated together, the front blaster,
the receiver capture on WT5CCP0, the combo-jack mode lines, the heartbeat,
contact polarity with the input shorted, and the user serial loopback.

Left:

- The LM3S image on the HC-800, and the writer on its bootloader.
- The HC-800's IR out and receiver on PD4 (carrier or envelope), and its gates:
  polarity, and which line goes with which carrier (both inferred in the decode).
- A write cut short by a power cut (the stub path).
- `reset-gpios` on the HC-800: its reset line is a `gpio_ich` pin that ACPI cannot
  name, so stock there is replaced only by a part already in its bootloader;
  openHC-to-openHC works without a line.
- `restore stock` end to end on each family (`restore iomcu` works on the EA-3).

Only then does a board set `firmware-name`.
