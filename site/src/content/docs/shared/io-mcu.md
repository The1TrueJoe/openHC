---
title: The IO microcontroller
description: The DLE/STX wire protocol, the bring-up handshake, the decoded per-board profile table, and IR in both directions.
sidebar:
  order: 3
---

The EA family's and HC family's IR jacks, relays, contacts and combo serial ports
do not hang off the SoC. They hang off a companion microcontroller reached over a
host UART.

| Family | Part | Core | Port | Baud |
|---|---|---|---|---|
| EA1 / EA3 / EA5 | **TM4C1231D5** (TI Tiva) | Cortex-M4F | `ttyS1` | **460800** |
| HC-800 / HC-250 | **LM3S1162** (TI Stellaris) | Cortex-M3 | `ttyS3` | 115200 |
| CA-1 | **none** | — | — | — |

Everything below is confirmed against a live EA1 unless marked otherwise. This is
groundwork for a clean-room replacement: **interfaces and observed behaviour only
— no vendor code is copied.**

Images, pulled from `/control4/firmware/io/` on a live EA1:

- app: `660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin` (65,536 B)
- bootloader: `..._Bootloader_Release_1.1.9.b6a901f.bin` (4,096 B)

`.flash.config` maps **ea1, ea3 and ea5 to the same pair**; amp1 gets a newer app;
hc800 and hc250 use the LM3S1162 images instead.

## The one thing that matters: 460800 baud

:::danger[The host link runs at 460800, not 115200]
The app image contains the constant `115200` **nineteen times**, sitting
immediately after the UART0/UART5/UART7 base addresses, which reads like proof.
It is not. Those are the bauds of the MCU's *own* user serial ports.

At 115200 the MCU is completely silent, it answers neither the application
protocol nor the bootloader, which looks exactly like dead or unpopulated
hardware. The giveaway is that **the bootloader autobauds**: it syncs to whatever
speed the host uses, so a wrong-speed host gets silence rather than garbage at
some other rate.
:::

Control4's own `ioserver` shows the sequence plainly with debug logging enabled:

```
Opening (/dev/ttySIO).
Set UART Speed (115200).        <- initial open only
STATE_MACHINE :: Set State from Unknown to bootloader
Set UART Speed (460800).        <- and everything after this is 460800
STATE_MACHINE :: negotiate bootloader speed
...
STATE_MACHINE :: Run Application
write_transport  Bytes To Write: 0x55 0x55
read_transport   17 Bytes Read: 0x10 0x02 0xd7 0x00 0x02 0x00 0x04 ...
```

## Bring-up sequence

Plain TI serial-bootloader protocol.

| Step | Bytes out | Reply |
|---|---|---|
| 1. pulse `/dev/gpio/io_reset` low→high | — | MCU enters bootloader |
| 2. autobaud | `55 55` | `00 cc` (0xCC = ACK) |
| 3. PING | `03 20 20` | `00 cc` |
| 4. GET_STATUS *(optional)* | `03 23 23` | `00 cc` + packet `03 40 40` (0x40 = SUCCESS) |
| 5. RUN app at 0x1000 | `07 32 22 00 00 10 00` | **none, by design** |
| 6. autobaud the app | `55 55` | `10 02 d7 ...` framed reply |

Two gotchas, both confirmed by experiment:

- **`COMMAND_RUN`'s address is big-endian, and RUN is never ACKed.** From
  `SendRunCommand` in the vendor's reflash library: the buffer is
  `0x22, a>>24, a>>16, a>>8, a`, and it passes `0` for the wait-for-ack argument
  where every other command passes `1`, the MCU has already jumped by the time an
  ACK could be sent. Waiting for one just times out.
- **The host must ACK (`0xCC`) any *packet* the MCU sends.** Leaving a GET_STATUS
  reply unacknowledged desyncs the bootloader and every subsequent command is
  silently ignored. This is what made an early big-endian RUN attempt look like a
  byte-order problem when it was a desync three steps earlier.
- **The autobaud knock goes first, before any DLE/STX frame.** The bootloader
  autobauds on the first bytes it hears. Ask "is the application up?" with a
  FIRMWARE_VERSION frame first, and the `55 55` after it gets no ACK at all.
  Measured on an HC-800 held in its bootloader by `io_reset`: frame first,
  silence; knock first, `00 cc`. A running application answers the same knock
  with an AUTO_BAUD_RESP frame (`10 02 d7 ...`), so one knock tells the two apart
  and leaves a part that is already up alone.
- **Reset with the driver unbound.** The same rule bites after a reset. Pulse
  `io_reset` under a bound driver and the contact poll's next frame reaches the
  fresh bootloader before any knock can, after which the part answers nothing
  until it is reset again. iod's `mcu.reset` (MQTT `cmd/mcu/reset`, or
  `POST /api/io/mcu/reset`) therefore unbinds the device from
  `/sys/bus/serial/drivers/ohc-iomcu`, pulses the line at board.env's confirmed
  polarity, binds it again and waits for a live relay read. The earlier version
  pulsed under a bound driver and judged the result by a contact read, which
  comes from the driver's cache: it reported `answering: true` for a part that
  then timed out every IR send. The fixed sequence was run twice on an HC-800,
  with IR, relays and contacts working afterwards each time.

### Who runs it: the kernel, at bind

`gpio-ohc-iomcu` is a serdev driver (`compatible = "control4,<part>-io-mcu",
"control4,io-mcu"`). It runs this sequence itself when it binds, then identifies
the part, sends it the board's pin table, and registers the gpiochip, the lirc
devices and the `/dev/ttyOHC<n>` ports. Nothing in userspace attaches it, and the
UART has no `/dev/ttyS<n>` of its own any more. The binding, the pin table and
openHC's own firmware are on [IO-MCU firmware](/shared/io-mcu-firmware/).

- **EA family:** a child of `uart1` (0x2F8, 460800) in the board device tree:
  `jack@N`, `ir-blaster`, `ir-receiver`, `relays` and `contacts` nodes, or the
  older count properties (`control4,relays`, `control4,ir-outputs`, …).
- **HC-800:** a `PRP0001` device under the BIOS's ttyS3 UART
  (`\_SB.PCI0.SBRG.UAR4`, 0x2E8, IRQ 10), from
  `board/hc800/acpi/ssdt-iomcu.asl`. It carries the same compatible, and the same
  child nodes as `_DSD` hierarchical data nodes, and is loaded as an ACPI table
  upgrade from the front of the initrd. ACPI rather than the board's device tree, because the UART belongs
  to ACPI: described in a tree it would lose its interrupt and be polled every
  ~8 ms, while a 16550A at 115200 fills its FIFO in under 2 ms.

The driver publishes what was declared at `/sys/class/ohc/iomcu`, and iod takes
its counts from there, not from board.env. See [IO as GPIO](/shared/io-as-gpio/).

**What has not run on hardware yet:**
- dropping the contact cache when a poll fails;
- the bring-up at bind on an EA (the EA1's MCU was already running when it was
  tested; the bootloader path was proven on the HC-800);
- the IO Extender taking its counts from the lines and lirc devices its kernel
  names.

### TI packet framing (bootloader)

```
[size = len + 2] [checksum] [data ...]     checksum = plain 8-bit sum of data
```

## Application protocol

```
DLE STX | opcode | seq | flags | len16 BE | payload... | checksum
10  02  |   a2   | 00  |  00   |  00 02   |  00 00    |    5c
```

- checksum = **negated 8-bit sum** of everything between `STX` and the checksum;
- any `0x10` in that range is escaped by doubling it (DLE stuffing);
- **the reply opcode is the request opcode + 1** (`0x24`→`0x25`, `0x54`→`0x55`,
  `0xa1`→`0xa4`, `0xd2`→`0xd7`), matching the vendor's own `*_GET`/`*_STATE`
  naming pairs.

### What this firmware actually implements

An opcode sweep of all 256 values against live app 1.0.36 found **only these
answering**:

| Request | Reply | Payload seen | Meaning |
|---|---|---|---|
| `0x24` | `0x25` | `c4:io_processor:c4-ir02` | PRODUCT_NAME |
| `0x34` | `0x35` | `1.0.36` | FIRMWARE_VERSION_GET |
| `0x54` | `0x55` | `00 00` | RELAY_GET |
| `0x56` | `0x55` | `00 00` | RELAY_TOGGLE / STATE_GET |
| `0x74` | `0x75` | `00 00 00 00` | CONTACT_GET (u32 bitmask) |
| `0xa1` | `0xa4` | `01` | UART_SEND → READY_FOR_DATA |
| `0xd2` | `0xd7` | `<u32 BE baud>` | AUTO_BAUD_GET — see below; NOT `00 07` + a u16 |

#### AUTO_BAUD_GET returns a 32-bit baud rate

This row was read wrong for a long time, and the HC-800 is what exposed it. The
EA's four payload bytes are `00 07 08 00`, which invites the reading "`00 07`,
then a 16-bit measurement" — and that is what this page used to say.

It is one **32-bit big-endian integer**, and it is the baud rate:

```
EA1/EA3   00 07 08 00  =  460800   exactly the link rate
HC-800    00 01 c2 07  =  115207   the link rate, MEASURED (nominal 115200)
```

The HC-800 is the giveaway: 115207 is 0.006% off nominal, so it is a measurement
rather than a constant — and it is meaningless as a "carrier period", while
being obviously right as a baud. Read the EA's the same way and `00 07 08 00`
lands exactly on its 460800. The leading `00 07` was never a header; it is the
top half of the number.

Captured from a live HC-800 with `ioserver` suspended:

```
--> 10 02 d2 13 00 00 00 1b        AUTO_BAUD_GET
<-- 10 02 d7 13 02 00 04 00 01 c2 07 46
```

Note also that this is the one opcode where **the reply is not request+1**:
`0xd2` answers `0xd7`, not `0xd3`.

#### The relay selector is an INDEX, not a bitmask

Settled on a live HC-800 by sweeping it. `RELAY_GET` echoes whatever selector it
is handed — `0x00` through `0x09` all come back verbatim — so a read can never
distinguish the two readings:

```
--> 10 02 54 11 00 00 01 …        RELAY_GET sel 0x01
<-- 10 02 55 11 00 00 02 01 00 …  [01, 00]
```

`RELAY_TOGGLE` can, because only a real relay moves:

| selector | toggling it |
|---|---|
| `0x00` `0x01` `0x02` `0x03` | flips exactly one relay each |
| `0x04` and above | does nothing |

So the four relays are addressed `0..3`, and the reply's first byte is the echoed
index rather than a mask.

An earlier reading took it for a bitmask and sent `1 << index`. That addressed
relays 1 and 2 — which worked — and turned indices 2 and 3 into `0x04` and
`0x08`, which address nothing. The board appeared to have two working relays and
two dead ones for as long as that assumption stood. It has four.

The `ff 00` an unselected query returns is consistent with this: `ff` is simply
an out-of-range index echoed back.

#### The HC-800 has the same empty query surface

The four documented-but-unimplemented queries were re-tested against the
LM3S1162, and all four are unimplemented there too — `0x12` IR_PIN_STATE_GET,
`0x42` IR_MODE_GET, `0x68` IROUT_STATUS and `0x94` CAPABILITIES_GET each
returned **nothing at all**. So the conclusion above holds for both families: a
host must treat a capabilities timeout as normal rather than as an error, and a
replacement firmware is free to drop unknown opcodes in silence, because that is
exactly what the stock ones do.

**`CAPABILITIES_GET` (0x94) isn't implemented**, nor are `IR_PIN_STATE_GET`
(0x12), `IR_MODE_GET` (0x42) or `IROUT_STATUS` (0x68) as queries, tried with both
an empty payload and a port index 0–5. The product string `c4-ir02` fits: a
minimal IR-focused build. So a host must treat a capabilities timeout as normal
rather than as an error.

Unsolicited frames on startup are a free hardware census, one per user serial
port:

```
10 02 a2 00 00 00 02 00 00 5c     UART_RECEIVE port 0
10 02 a2 01 00 00 02 01 00 5a     UART_RECEIVE port 1
```

An EA1 sends two; an EA3 sends three.

## User serial — confirmed on an EA3

The combo jacks' serial mode was settled against a live EA3 (ldisc detached,
raw app protocol on `ttyS1` @460800), with port 0 looped back so one write
proved the whole chain end to end:

| Request | Reply | Payload | Meaning |
|---|---|---|---|
| `0xa0` | *(none)* | `[port, 1, baud_index, data_bits, parity, stop_bits]` | UART_SET_CONTROL — switch a jack to serial + set line |
| `0xa1` | *(none)* | `[port, data…]` | UART_SEND — transmit bytes |
| `0xa2` | *(async)* | `[port, data…]` | UART_RECEIVE — bytes that arrived |

All three are **fire-and-forget**: `UART_SET_CONTROL` and `UART_SEND` get no
reply, and received bytes arrive unsolicited as `UART_RECEIVE`. `baud_index`
0–6 is `1200/2400/4800/9600/19200/38400/57600`; **anything else is 115200**.
`port` is 0-based (port 0 is the jack labelled "Serial 1").

```
--> 10 02 a0 .. 00 01 03 08 00 01     UART_SET_CONTROL port 0 -> 9600 8N1
--> 10 02 a1 .. 00 41 54 0d           UART_SEND       port 0, "AT\r"
<-- 10 02 a2 03 00 00 04 00 41 54 0d  UART_RECEIVE     port 0, "AT\r"   (looped back)
```

:::note[There is no READY_FOR_DATA handshake on the normal path]
An earlier opcode sweep recorded `0xa1 → 0xa4` (`READY_FOR_DATA`), which read
like a two-step flow-control handshake. It is not: a bare `UART_SEND [port]`
with no data returns nothing, and `UART_SEND [port, data…]` transmits
immediately. Send the bytes in the one frame.
:::

**openHC bridges this in the kernel, not in iod.** The `gpio-ohc-iomcu`
driver owns the UART (a serdev device), so nothing in userspace can speak these
opcodes on it directly. The driver therefore mux-es each MCU-routed port onto a
`/dev/ttyOHC<n>` tty — the n_gsm pattern — and iod's ordinary host-UART bridge
opens that.

### Combo jacks

Some rear jacks are an IR output and an MCU serial port on one connector, used
one way at a time: IR 1–2 on the EA-1 (serial 1–2), IR 1–3 on the EA-3
(serial 1–3), IR 1–2 on the EA-5 (serial 3–4). Each model's device tree
(`ea1.dtsi`, `ea3.dtsi`, `ea5.dtsi`) lists its jacks under `&io_mcu`:

```dts
jack@1 {
	reg = <1>;			/* panel number */
	label = "IR / Serial 1";
	control4,ir-channel = <0>;	/* IROUT_SEND bit */
	control4,uart = <0>;		/* MCU user UART, ttyOHC0 */
};
```

A jack with both is a combo jack. Its mode lives in the kernel, in
`/sys/class/tty/ttyOHC<n>/mode` (`ir` or `serial`, next to `jack` and `label`).
It starts as `ir`. In `ir`, opening the tty fails with `EBUSY`; in `serial`, its
IR output's lirc device refuses to send with `EBUSY`. Going back to `ir` sends
`UART_SET_CONTROL` with enable 0, and is refused while the tty is open.

iod sets it with `PUT /api/io/jacks/<jack>` `{"mode": "serial"}`, saves it, and
puts it back at boot. A combo jack left as IR gets no serial session; one set to
serial is opened and held like any other port. Capabilities list the jacks
under `ir.jacks` with their current mode.

Each user UART also owns two mode lines, A and B
([decoded](/shared/io-mcu-pinmap/#combo-jack-lines)): both low is IR, A high is
serial, B high serial with RX and TX swapped (`0xa3`, stock's null-modem option).
Unconfirmed: that enable 0 hands the jack back to IR, and what B does on the wire.

Boards without jack nodes (the HC-800's ACPI table) still give
`control4,ir-outputs` and `control4,uarts` counts: IR jacks 1..N on channels
0..N-1, UARTs on connectors of their own.

## The per-board profile table

One image serves ea1, ea3 and ea5. It does that with a table of **six board
profiles at file offset `0x1fec`, stride `0x4a4`**, selected at runtime. This table
has been fully decoded and is the authoritative source for per-board IR and serial
counts. Every pin, per board, is on [IO-MCU pin maps](/shared/io-mcu-pinmap/).

Block layout (full table on the pin-map page):

```
+0x000  0x18   IR receiver descriptor
+0x018  0x1c   front blaster (gets the LAST channel index)
+0x034  0x1c   rear IR outputs, up to 16 slots, list ends at the first unpopulated
+0x1f4         contacts (inputs)     +0x2f4  relays (outputs)
+0x37c         heartbeat LED         +0x384  combo-jack mux lines A, B
+0x3b4         peripheral records, 0x28 each, first is the host link
```

An **IR output descriptor**:

```
+0x00  u32  GPIO port base (APB)
+0x04  u32  pin mask in bits 0..7;  BIT 8 = CHANNEL IS POPULATED ON THIS BOARD
+0x08  u32  timer base
+0x0c  u32  exception number for timer A  (= IRQ + 16)
+0x10  u32  exception number for timer B, or 0xFF
```

A **UART record**:

```
+0x00  u32  flags        0x00010001 host link, 0x02000001 user port
+0x04  u32  UART base
+0x08  u32  PCTL value, RX      +0x0c  u32  PCTL value, TX
+0x10  u32  exception number  (= IRQ + 16)
+0x14  u32  default baud      (0x0001c200 = 115200 on every board)
+0x18  u32  GPIO base, RX      +0x1c  u32  RX pin mask
+0x20  u32  GPIO base, TX      +0x24  u32  TX pin mask
```

### Bit 8 is the key

Decoding bit 8 of the pin-mask word as *populated* is what makes the table
resolve. Without it the six blocks look like arbitrary reorderings; with it they
become board profiles, and the counts match physical hardware exactly:

| Block | IR outputs populated | User UARTs | Board |
|---|---|---|---|
| 0 | 9 | 2 | unidentified (same as 1 and 5) |
| 1 | 9 | 2 | unidentified (same as 0 and 5) |
| **2** | **5** | **2** | **EA1**: 4 rear jacks + 1 internal blaster |
| **3** | **7** | **3** | **EA3**: 6 rear jacks + 1 internal blaster |
| 4 | 9 | 2 | **EA5** (inferred): rear jacks 5–8 in EA1/EA3 order |
| 5 | 9 | 2 | unidentified (same as 0 and 1) |

Two independent facts confirm the EA3 assignment: the owner counts **6 IR jacks**,
and a live EA3's `ioserver` opens **three** user-serial sockets where an EA1 opens
two.

### The decoded profiles

Channels are listed in `IROUT_SEND` `output_mask` bit order. The receiver is
`PD6 / WTIMER5A`, identical in all six blocks. The rear records are numbered first
and the `+0x018` record, `PD4`, takes the next index, so the front blaster is the
last bit, not bit 0 (an earlier version of this table had that backwards).

```
bit  pin  timer  irq    EA1 (block 2)   EA3 (block 3)
0    PB6  T0      19    jack 1          jack 1
1    PF2  T1      21    jack 2          jack 2
2    PB0  T2      23    jack 3          jack 3
3    PB2  T3      35    jack 4          jack 4
4    PD0  WT2     98    —               jack 5
5    PD2  WT3    100    —               jack 6
-    PC4  WT0     94    absent          absent   (PC4 is UART4 RX on EA3)
-    PC6  WT1     96    absent          absent
4/6  PD4  WT4    102    front blaster   front blaster   (bit 4 on EA1, 6 on EA3)
```

```
UART   pins        irq   EA1 (block 2)   EA3 (block 3)
UART0  PA0/PA1       5   host link       host link
UART5  PE4/PE5      61   user port 1     user port 1
UART7  PE0/PE1      63   user port 2     user port 2
UART4  PC4/PC5      60   —               user port 3
```

Both boards populate a **contiguous run of rear channels from 0**, and the
blaster follows it, so `output_mask` is dense: `0x1f` on EA1, `0x7f` on EA3.

### IROUT_SEND, and the field that hard-faults the part

Decoded from the vendor image and verified on air with a Global Caché learner.
The payload is **14 fixed bytes** followed by `(len - 14) / 2` duration words:

| field | size | meaning |
|---|---|---|
| 1 | u8 | mode; `< 2` skips the repeat block entirely |
| 2-4 | u8×3 | 24-bit output **bitmask** — bit N is the jack labelled N+1 |
| 5-6 | u16×2 | unread on the simple path |
| 7 | u16 | **Pronto carrier word** |
| 8-9 | u16×2 | repeat count, repeat offset |
| 10+ | u16[] | burst durations; the firmware sets bit 15 on marks itself |

It answers opcode `0x68` with a one-byte status, 0 for success.

:::danger[A carrier word of 0 is not an error, it is a brick]
The handler computes the carrier frequency by dividing:

```
ldr   r2, [pc, #0x1c]      ; 4,145,146 = 1e6 / 0.241246, the Pronto constant
udiv  r1, r2, r1           ; ÷ the carrier field
```

Zero there is a divide-by-zero, and with `DIV_0_TRP` set the Cortex-M3 takes a
UsageFault → HardFault. The part then answers **nothing** — not identify, not
relays, not contacts — and survives a daemon restart and a full reboot. Only a
power cycle brings it back.

This was originally mistaken for a payload-length limit, because the first code
that triggered it happened to be long. It is not: a six-word code with a zero
carrier kills it just as dead, and a 150-byte code with a valid carrier is fine.
:::

The output selector being a bitmask rather than an index matters: `1 << port`,
not `port`. Sending an index addresses the wrong jack, or none.

On an HC-800 the mask is **bit 0..5 = the six rear jacks, bit 6 = the internal
front blaster**, and nothing else in the 24 bits does anything. So the front
emitter is not "jack 7" — it is a different piece of hardware with no socket
behind it, which is why openHC addresses it by name (`ir/front/send`) rather
than by a number that would send somebody hunting for a seventh connector. The
front receiver sits on the same panel, so its captures come back on
`ir/front/rx`.

Verified on an HC-800 — bit 0 out of jack 1 and bit 5 out of jack 6, both
returning the carrier and every duration we sent, to within rounding:

```
GC-IRL,38000,343,171,23,22B,22,64BBBBBCCBCCCCCBBBCBBBBCCCBCCCC,22,3806
```

The learner speaks at **9600 baud**, not 115200.

### The contact mask is LITTLE-endian

`CONTACT_GET` answers a u32 in which **byte 0 carries contacts 1-8**, bit 0 being
the first contact and 1 meaning CLOSED.

This was read as big-endian for a long time and nothing caught it, because a
board with every contact open answers `00 00 00 00` — identical either way.
Closing contact 4 on an HC-800 answers:

```
08 00 00 00
```

Little-endian that is bit 3, the fourth contact. Big-endian it would be bit 27,
which is not a contact any of this hardware has.

Note the AUTO_BAUD response really is big-endian, so the two genuinely differ;
do not "fix" one to match the other.

### Relays and contacts fall out of the same table

Between the IR outputs and the UART records each block carries a run of plain
`[gpio_base, pin_mask|attr]` pairs, and bit 8 means the same thing. Two groups of
four sit at the front:

```
group        pins                    EA1 (block 2)   EA3 (block 3)   blocks 0,1,4,5
first four   PF0 PF1 PF3 PF4         none            PF0 only        all four
second four  PA2 PA3 PA4 PA5         none            PA2 only        all four
then         PC7 PE2 PE3 PB1 PB3     populated       populated       populated
EA3 extras   PA7, PB5                —               populated       —
```

This reproduces the known hardware exactly, which is strong confirmation the
block→board assignment is right:

- **EA1 populates none of the eight**, and an EA1 has no relays and no contacts.
- **EA3 populates exactly one from each group**, and an EA3 has one relay and one
  contact, owner-confirmed off the PCB.
- The 9-output blocks populate all eight, i.e. **4 relays + 4 contacts**, which is
  the EA5 complement.

**The first group is contacts and the second is relays.** The init code sets
PF0/PF1/PF3/PF4 up as GPIO inputs, and the contact poll reads them (closed = pin
low). PA2–PA5 become outputs driven low, and relay-on drives them high. `PC7` is a
heartbeat. PE2/PE3 and PB1/PB3 (plus PA7/PB5 on EA3) are each user UART's pair of
combo-jack mux lines. Details and evidence are on
[IO-MCU pin maps](/shared/io-mcu-pinmap/).

### The selector, disassembled

The vendor picks its block from an analogue board-ID strap. That function lives at
flash address **`0x1878`** (an earlier version of this page said `0x2878`). Its
literal pool sits immediately before the profile table, which is how it was found.
It configures ADC0 sequencer 0 with 8× hardware oversampling, makes one step on
channel 10 (AIN10 = PB4), runs five conversions, and keeps only the last. It then
divides by 250, rounding up when the remainder is 126 or more.

Two corrections to an earlier reading of this ("five samples averaged"):

- **The five conversions are a settle-and-discard loop, not an average.** The
  scratch word is *overwritten* by every `ADCSequenceDataGet`; nothing sums it.
  Only the fifth reading reaches the arithmetic.
- **The averaging is in hardware** — `ADCHardwareOversampleConfigure(ADC0, 8)`
  makes each returned sample an 8× average.

The id is used directly as the block index; 21 separate sites in the image
multiply it by the `0x4a4` stride. So the strap mapping is fully determined, on a
12-bit ADC against 3.3 V:

| Board id / block | ADC counts | Volts on PB4 | Board |
|---|---|---|---|
| 2 | ~500 | ~0.40 V | EA1 |
| 3 | ~750 | ~0.60 V | EA3 |
| 4 | ~1000 | ~0.81 V | EA5 (inferred) |
| N | N × 250 | N × 0.20 V | — |

There is no range check: an id of 6 or more would index past the table.

**Those voltages are predictions from the decoded arithmetic, not measurements.**
Nobody has put a meter on PB4. openHC's firmware does not need them: it reads no
strap and carries no board table, and takes every pin from the host.

## Contacts — confirmed on an EA3

`CONTACT_GET`'s payload is a **32-bit bitmask, one bit per contact, bit N =
contact N, and a CLOSED contact reads 1.** Verified by shorting the input and
watching both edges:

```
jumper OUT   set_state    - Contact State : New State (0x00000000)
jumper IN    set_state    - Contact State : New State (0x00000001)
jumper OUT   update_state - Current state: (0x00000001) New State (0x00000000)
                            Send update of state for contact (0), now (open)
```

Both directions, with an independent fresh read after each change, a measured
edge rather than a single snapshot. The EA3 populates exactly one contact and it
is **index 0**, consistent with the profile table.

### Contact state is pulled, never pushed

:::caution[This is the part that matters for a replacement daemon]
The MCU does **not** volunteer contact changes, and `ioserver` doesn't poll on a
timer of its own — it re-reads only when its director asks for the `c4.hc.cs` MIB.

In a 45-second idle window with the contact held closed, **not one** contact line
appeared; the observed polls were roughly one to two minutes apart.

So a replacement daemon must run its own `CONTACT_GET` poll loop and derive edges
by comparison. Waiting for an unsolicited frame will simply never fire.
:::

Related MIBs on the same path: `c4.hc.cs` (contact state), `c4.hc.rs` (relay
state), `c4.hc.fwv` (firmware version).

**Bound to a pin since:** the EA3's contact 0 is `PF0`, a GPIO input (and a
locked pin the stock firmware unlocks at boot). The relay is `PA2`, an output. See
[IO-MCU pin maps](/shared/io-mcu-pinmap/#ea3--block-3).

## IR output — confirmed

`IROUT_SEND` is opcode `0x66`, answered by `0x68`. The payload is a **6-byte
header followed by the raw Pronto/CCF words verbatim, big-endian**:

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | u8 | `repeat_count` | `0xFF` = repeat until `IROUT_STOP_RAMP` (0x69). Stock ioserver hardcodes `0xFF`. |
| 1..3 | u24 BE | `output_mask` | bit *N* = IR output *N*. **Not an index — a mask.** |
| 4..5 | u16 BE | `code_id` | caller-assigned handle |
| 6..7 | u16 BE | Pronto word 0 (type) | always `0x0000`; **the MCU never reads it** |
| 8..9 | u16 BE | Pronto word 1 (carrier) | a divisor, **not Hz**: `Hz = 4145146 / value` |
| 10..11 | u16 BE | Pronto word 2 | intro burst-**pair** count |
| 12..13 | u16 BE | Pronto word 3 | repeat-sequence burst-**pair** count |
| 14+2i | u16 BE | burst words | **carrier-period counts**, alternating mark/space |

So `ioserver` does essentially no conversion: it validates the Pronto string and
passes the words straight through. **Burst durations are carrier periods, not
microseconds.**

Verified on a live EA1, this exact frame emitted 38 kHz IR on output 1:

```
TX 10 02 66 11 00 00 16 01 00 00 01 00 01 00 00 00 6d 00 02 00 00 01 57 00 ac 00 16 00 16 d1
   (repeat=1, mask=output 1, code_id=1, Pronto 0000 006D 0002 0000 0157 00AC 0016 0016)
RX 10 02 68 11 02 ... 00     IROUT_STATUS, seq echoes 0x11, flags=2, status 0
RX 10 02 68 02 00 ... 04     async follow-up, status 4 — meaning not yet pinned down
```

How it was established: five independent analyses — `ioserver`'s Pronto packer,
its message serialiser, the Tiva `0x66` handler in Thumb-2, the HC800 LM3S1162
image as cross-reference, and a documentation sweep — each attacked by three
adversarial verifiers. **The sender-side and receiver-side analyses agreeing
independently is what makes the layout trustworthy**; the documentation sweep
found nothing and its guesses were correctly refuted.

What static analysis establishes about the layer above: only Pronto code type
`0000` is accepted, carrier bounds are **20,000 Hz min / 459,995 Hz max**, **max
256 burst pairs**, and the reported pair count is cross-checked against the parsed
count.

**Still open:** the semantics of the async `0x68` status value; whether bit 14
(`0x4000`) in a burst word is a long-duration escape (both firmwares appear to
treat values `> 0x3FFF` specially, so raw Pronto words that large may need
splitting); and what payload bytes 6..7 were originally for.

## IR input — working

The EA1 has a front IR receiver, separate from the blaster and the four output
jacks, and it **is** wired to the Tiva.

### The enable sequence — order matters

```
1. IRIN_SET_CAPTURE_TO_INIT  0x77, zero-length payload
2. IR_PIN_STATE_SET          0x11, 6-byte payload: [0]=0, [1]=0, [2]=pin mask, [3..5]=0
3. IR_MODE_SET               0x41, 1 byte, (mode & 0x30) in {0x30, 0x20}
```

`0x77` is **not an arm.** Despite the name it *resets* capture state, and among
other things it zeroes the input pin mask, so on its own it enables nothing.
`0x11` sets that mask. payload[0] must stay **0**: the capture engine only runs its
timing math while that byte is zero.

:::danger[Never re-arm mid-listen]
Another `0x77` wipes the pin mask and capture goes silent. An early version of the
probe tool re-armed every 3 seconds, which disabled capture continuously and
produced **five consecutive false negatives** before anyone suspected the tool
rather than the hardware.
:::

### `IRIN_CAPTURED` (0x97) payload

```
u16 BE   carrier period, in MCU timer ticks
u16 BE   burst words, repeated:
             bit 15 set = carrier ON (mark), clear = space
             bits 0..13 = duration in CARRIER PERIODS
```

Long codes are **split across several frames**, each repeating the 2-byte carrier
header, with the sequence number incrementing.

Verified against a real NEC remote — 40 frames, all checksums good:

```
0515 | 0001 8156 00aa 8016 0016 8014 003f 8016 0016 ...
^^^^   carrier period = 1301 ticks
       M342  S170  M22  S22  M20  S63 ...
```

`342 periods / 9 ms` = **38000 Hz exactly**, and the cells decode as textbook NEC:
a 9.0 ms + 4.47 ms leader, then 0.58 ms marks with 0.58/1.66 ms spaces. NEC
specifies 9.0/4.5, 560 µs and 1690 µs.

**This independently confirms the `IROUT_SEND` unit derivation**: durations are
carrier periods, not microseconds, big-endian u16, in both directions. Note the one
asymmetry, capture flags marks explicitly with bit 15, whereas `IROUT_SEND` takes
raw Pronto words where mark/space is positional.

## Why vendor `ioserver` could not be used as an oracle

Driving Control4's own `ioserver` to emit IR would log the exact frames, since its
debug options hex-dump everything. It is **not** available: its ports 20000 and
5100 are mutual-TLS and require a client certificate signed by Control4's product
CA. The device's own self-signed certificate completes the handshake but
`ioserver` still rejects the session.

## Flash layout

```
part: TM4C1231D5PM — 64 KB flash, 24 KB SRAM

0x0000 .. 0x0FFF   bootloader (4 KB). In the app image this region is all 0xFF.
0x1000             app vector table:  initial SP 0x2000560C, reset PC 0x00008635
0x1000 .. 0xFFFF   application  (60 KB usable)
```

A flash write of the app starts at `0x1000` and leaves the bootloader intact,
which is what makes bad app images recoverable. `io_start_address 4096` in
`/etc/ioserver_config.conf` is exactly this address.

## Silicon facts that would have produced silent failures

Recorded because each of these makes firmware that fails with no diagnostic.

- **TM4C1231D5PM is 64 KB flash / 24 KB SRAM**, not the 256 KB/32 KB of larger
  TM4C123 parts. A linker script with the bigger numbers puts `_estack` past the
  top of real SRAM and the firmware hard-faults on its **first push**, before any
  code can report it. (The stock image spans `0x0000..0xFFFF` and an SP of
  `0x2000560C` rules out a 12 KB part; libopencm3's device database agrees.)
- **This part has no PWM module.** DC1/DC3/DC5 PWM-presence bits read 0. The IR
  carrier must come from a general-purpose timer's CCP pin.
- **GPIO AHB apertures are unmapped at reset**, until the matching bit is set in
  `SYSCTL_GPIOHBCTL` (`0x400FE06C`). A UART muxed through `0x4005_8000+` without
  that simply never comes up, reads return zero, writes vanish, nothing errors.
  Use the APB apertures; the stock application uses APB 238 times to AHB 12.
- **The bootloader doesn't set VTOR.** The 4 KB TI bootloader contains no
  reference to `0xE000ED08`, so an application at `0x1000` must relocate its own
  vector table.
- **A complete vector table is 123 entries** (16 system + IRQ 0..106). A short
  table doesn't fault, peripheral interrupts just vector into whatever code
  follows it in flash.
- **`RCC2.SYSDIV2` is 7 bits when `DIV400` is set.** Masking 6 bits leaves bit 28
  stale, which works from reset only because RCC2's reset value happens to have it
  clear.
- **Burst timing needs a 32-bit timer.** Worst case is 16383 carrier periods ×
  2500 ticks (a max-length burst at the 20 kHz minimum carrier) = 819 ms, which
  fits neither a 16-bit GPTM nor a 16+8 prescaled one.
- **Two errata shape the design.** GPTM#11: in Input Edge-Time count-up mode the
  prescaler misbehaves if `GPTMTnILR` is loaded with `0xFFFF`, never write that
  value on the capture path. GPTM#10: writing `GPTMTnMATCHR`/`GPTMTnPR` on an
  enabled timer perturbs the counter in RTC and edge-count modes, with no
  workaround.

## The IR channel pin map, and how the package was settled

The vendor image carries a 10-entry descriptor table at flash **`0x2490`**, stride
**`0x1c`**. It is profile block 1: the receiver, the front blaster, then eight
rear records. The `ch` column below is record order, **not** the `IROUT_SEND` bit.
For that, see [IO-MCU pin maps](/shared/io-mcu-pinmap/).

| rec | pin | timer | CCP | IRQ A |
|---|---|---|---|---|
| 0 | PD6 | WTIMER5 | WT5CCP0 | 104 |
| 1 | PD4 | WTIMER4 | WT4CCP0 | 102 |
| 2 | PB6 | TIMER0 | T0CCP0 | 19 |
| 3 | PF2 | TIMER1 | T1CCP0 | 21 |
| 4 | PB0 | TIMER2 | T2CCP0 | 23 |
| 5 | PB2 | TIMER3 | T3CCP0 | 35 |
| 6 | PC4 | WTIMER0 | WT0CCP0 | 94 |
| 7 | PC6 | WTIMER1 | WT1CCP0 | 96 |
| 8 | PD0 | WTIMER2 | WT2CCP0 | 98 |
| 9 | PD2 | WTIMER3 | WT3CCP0 | 100 |

**All ten land exactly on the documented CCP0 pin for their own timer**, which is
what confirms the decoding, a 10/10 correlation isn't coincidence. The exception
numbers corroborate it independently.

Two things this settles:

- **The package is PM.** Channel 2 uses PB6 = T0CCP0, which exists on
  TM4C1231D5PM and **not** on the PZ variant. (Confirm on any given unit by
  reading `SYSCTL_DID1` and masking `PRTNO`: `0x19` = PM, `0x36` = PZ.)
- **The carrier must come from a GPTM CCP output**, consistent with this part
  having no PWM module at all.

**Which physical jack is which bit is the DT's convention (jack N = bit N−1).**
It's proven on the HC-800 but not yet on an EA. Drive one bit at a time and watch
which emitter lights.

## The HC-800

An earlier version of this page said the HC-800's LM3S1162 image "uses only
TIMER0–3 and all three UARTs — UART0 host, UART1/UART2 the two user ports", and
flagged both halves as contradicted by a live unit. Both are now settled, and
neither by the reading that produced them.

### The two user serial ports are HOST UARTs

`ioserver` on a running HC-800 holds **three** ports at once:

```
ioserver(2791) -> /dev/ttyS3     the LM3S1162
ioserver(2791) -> /dev/ttyS1     0x2f8, RTS|DTR
ioserver(2791) -> /dev/ttyS2     0x3e8, RTS|DTR
```

`ttyS1` and `ttyS2` are real 16550As on the LPC bus, opened and configured
directly by the daemon — note the asserted modem lines, where the never-opened
Zigbee port at `ttyS4` shows no flags at all. The rear RS-232 jacks are wired to
the host, not routed through the MCU.

So why does the image configure three UARTs? Because **it is the same binary as
the HC-250**: `.flash.config` maps `hc800` and `hc250` to one pair of files.
"MultiConfig" means multi-**board**. Like the EA image, it reads an analogue strap
(ADC channel 0, `round(adc / 85)`, routine `0x519c`) and uses the result as the
index of one of four config blocks. The HC-800 is block 2 and the HC-250 is
block 1. The `Processor: LM3S…` strings only label a version banner. An earlier
version of this page read them as the block selector, which was wrong. A feature
present in the image is not necessarily a feature of this board.

**Consequence for a replacement firmware:** the HC-800's MCU does IR, 4 relays
and 4 contacts, and no user serial at all.

### Six IR outputs, gated onto two PWM carriers

The old figure came from counting timer base addresses, which is a lower bound
rather than a count. The vector table settles it. The stock image's table is 46
entries (16 system + 30 IRQs) and only six handlers differ from the common
default at `0x7783`:

| IRQ | Peripheral |
|---|---|
| 5, 6 | UART0, UART1 |
| 9 | PWM **Fault** |
| 10, 11, 12 | PWM generators **0, 1, 2** |

**This part has a PWM module and the firmware uses it** — the module's base
`0x40028000` sits at flash `0x4BA0`, immediately beside those handlers. Only two
PWM pins are used, though: PWM1 on `PD1` (generator 0) and PWM5 on `PF3`
(generator 2). Each of the seven outputs (six jacks + the front blaster) has its
own pair of GPIO gate lines that connect it to one carrier or the other. That is
the sharpest difference from the EA's TM4C1231D5, which has no PWM module and gives
every jack its own timer CCP.

The static vector table claims no timer, GPIO or SysTick interrupt. Contacts are
polled. The receiver's record does name TIMER1B (CCP3 on `PD4`), so its handler
is presumably registered at runtime.

### The pin map

Decoded: the jack gates, carriers, receiver, relays (PA5, PA4, PA3, PA2), contacts
(PB0–PB3) and heartbeat (PF1), with the block layout and evidence, on
[IO-MCU pin maps](/shared/io-mcu-pinmap/#hc-800--block-2). An earlier reading
here took `PC7 PC6 PF4 PC5 PC4 PF5` for six IR outputs. They are the first two
jacks' three-line gate groups.

openHC's firmware drives none of these until the host's pin table names them, and
the HC-800's ACPI table gives no pins until the gate polarity and relay order are
checked on hardware. A relay here may be switching a real load.

### The protocol is confirmed on this part

Asked directly, on `/dev/ttyS3` of a live unit with `ioserver` suspended:

```
--> 10 02 34 01 00 00 00 cb                     FIRMWARE_VERSION_GET
<-- 10 02 35 01 02 00 08 "03.26.15"         33
--> 10 02 24 02 00 00 00 da                     PRODUCT_NAME
<-- 10 02 25 02 02 00 1b "c4:ir_processor:c4-ir01-i2c" 6c
```

Reply opcode = request + 1, flags bit 1 = response, checksum = negated 8-bit
sum — the same protocol this page documents for the EA, decoded by the same
`ohc_proto.c` without modification. **The host link is 115200 on this board**,
not the EA's 460800. The reported version matches the extracted image exactly,
so the running firmware is the file in `/control4/firmware/io/`.

### Image container

The LM3S application file is wrapped, where the TM4C ones are raw:

```
[0x000 .. 0x0FF]      256-byte Control4 text header, 0xFF padded
[0x100 .. 0x100FF]    64 KB flash image (low 4 KB left erased — the bootloader)
[0x10100 .. 0x10101]  CRC-16/ARC of the whole 64 KB, little-endian
```

CRC-16/ARC: poly `0x8005`, init 0, reflected in and out, no final xor. openHC's
`tools/mkimage.py` reproduces the stock image's stored `0xa578` exactly, which
is what makes the format trustworthy enough to flash against.
