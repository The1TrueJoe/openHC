# IO MCU — protocol, bring-up, and firmware notes

The EA1's IO processor is a **TI Tiva TM4C1231D5**. It drives the IR jacks and
the two combo IR/serial ports, and it is reached over the host UART.

Everything below is confirmed against a live EA1 unless marked `?`. This is
groundwork for a clean-room replacement firmware: **interfaces and observed
behaviour only — no vendor code is copied into openHomeController.**

Images (pulled from `/control4/firmware/io/` on a live EA1):
* app: `660-00063_TM4C1231D5_IO_Processor_Release_1.0.36.20ebb2d.bin` (65,536 B)
* bootloader: `660-00063_TM4C1231D5_IO_Processor_Bootloader_Release_1.1.9.b6a901f.bin` (4,096 B)

`.flash.config` maps **ea1, ea3, ea5 → the same pair**; amp1 gets a newer app
(1.0.41); hc800/hc250 use the LM3S1162 images instead.

## The one thing that matters: 460800 baud

> **The host link runs at 460800 baud, not 115200.**

This cost days of dead ends, so it is worth being explicit about the trap. The
app image contains the constant `115200` **19 times**, sitting immediately after
the UART0/UART5/UART7 base addresses — which reads like proof that the link is
115200. It is not. Those are the bauds of the MCU's *own* user serial ports
(UART5/UART7, the two combo jacks). The **host** link on UART0 is 460800.

At 115200 the MCU is completely silent — it answers neither the application
protocol nor the bootloader — which looks exactly like a dead or unpopulated
part. It is neither. The giveaway is that the bootloader **autobauds**: it syncs
to whatever speed the host uses, so a wrong-speed host gets silence rather than
garbage at some other rate.

Control4's own `ioserver` shows the sequence plainly with `log4cplus.rootLogger
= DEBUG` enabled in `/etc/logging/ioserver.conf`:

```
Opening (/dev/ttySIO).
Set UART Speed (115200).        <- initial open only
STATE_MACHINE :: Set State from Unknown to bootloader
Set UART Speed (460800).        <- and everything after this is 460800
STATE_MACHINE :: negotiate bootloader speed
...
STATE_MACHINE :: Run Application
Set UART Speed (460800).
STATE_MACHINE :: negotiate application speed
write_transport  Bytes To Write: 0x55 0x55
read_transport   17 Bytes Read: 0x10 0x02 0xd7 0x00 0x02 0x00 0x04 ...
```

## Which port

`/dev/ttySIO` → `/dev/ttyS1`, created by `/etc/rc.d/99control4`, whose own
comment settles it:

```
# Ninjago : ttyS0   - Console
#           ttyS1   - Tiva IO Chip
#           ttyS2   - 8051 Power Management Inside CE53xx   <- wrong; it is a PIC24
```

The EA1's CP2104 USB-serial bridge exposes **only** Zigbee — see
`handle_ninjago()` in `/etc/udev/rules.d/setup_usb_serial_ports.sh`, which links
interface 00 to `/dev/ttySZigbee` and nothing else. So the two user serial ports
are MCU-routed, not host tty nodes.

## Bring-up sequence

`overlay/services/ohc-io` implements this; it is plain TI serial-bootloader
protocol.

| Step | Bytes out | Reply |
|---|---|---|
| 1. pulse `/dev/gpio/io_reset` low→high | — | MCU enters bootloader |
| 2. autobaud | `55 55` | `00 cc` (0xCC = ACK) |
| 3. PING | `03 20 20` | `00 cc` |
| 4. GET_STATUS *(optional)* | `03 23 23` | `00 cc` + packet `03 40 40` (0x40 = SUCCESS) |
| 5. RUN app at 0x1000 | `07 32 22 00 00 10 00` | **none by design** |
| 6. autobaud the app | `55 55` | `10 02 d7 ...` framed reply |

Two gotchas, both confirmed by experiment:

* **`COMMAND_RUN`'s address is big-endian, and RUN is never ACKed.** From
  `SendRunCommand` in `libioproc_reflash_ea.so`: the buffer is
  `0x22, a>>24, a>>16, a>>8, a`, and it passes `0` for `SendPacket`'s
  wait-for-ack argument (every other command passes `1`) — the MCU has already
  jumped by the time an ACK could be sent. Waiting for one just times out.
* **The host must ACK (`0xCC`) any *packet* the MCU sends.** Leaving a
  GET_STATUS reply unacknowledged desyncs the bootloader and every subsequent
  command is silently ignored. This is what made an early big-endian RUN attempt
  look like a byte-order problem when it was a desync.

### TI packet framing (bootloader)

From `SendPacket`/`CheckSum` in `libioproc_reflash_ea.so`:

```
[size = len + 2] [checksum] [data ...]     checksum = plain 8-bit sum of data
```

The host driver performs steps 1–6 at boot, so it can then assume the MCU is in application mode.

## Application protocol

```
DLE STX | opcode | seq | flags | len16 BE | payload... | checksum
10  02  |   a2   | 00  |  00   |  00 02   |  00 00    |    5c
```

* checksum = **negated 8-bit sum** of everything between `STX` and the checksum
* any `0x10` in that range is escaped by doubling it (DLE stuffing)
* **the reply opcode is the request opcode + 1** (`0x24`→`0x25`,
  `0x54`→`0x55`, `0xa1`→`0xa4`, `0xd2`→`0xd7`), which matches the vendor's own
  `*_GET`/`*_STATE` naming pairs

### What this firmware actually implements

An opcode sweep of all 256 values against live app 1.0.36 (`ohc-ioprobe.py
--scan`) found **only these answering**:

| Request | Reply | Payload seen | Meaning |
|---|---|---|---|
| `0x24` | `0x25` | `c4:io_processor:c4-ir02` | PRODUCT_NAME |
| `0x34` | `0x35` | `1.0.36` | FIRMWARE_VERSION_GET |
| `0x54` | `0x55` | `00 00` | RELAY_GET |
| `0x56` | `0x55` | `00 00` | RELAY_TOGGLE/STATE_GET |
| `0x74` | `0x75` | `00 00 00 00` | CONTACT_GET |
| `0xa1` | `0xa4` | `01` | UART_SEND → READY_FOR_DATA |
| `0xd2` | `0xd7` | `00 07 <u16>` | AUTO_BAUD_GET (the trailing u16 varies per sync — a timing measurement) |

**`CAPABILITIES_GET` (0x94) is not implemented**, nor are `IR_PIN_STATE_GET`
(0x12), `IR_MODE_GET` (0x42) or `IROUT_STATUS` (0x68) as queries — tried with
both an empty payload and a port index 0–5. The product string `c4-ir02` fits: a
minimal IR-focused build with no LEDs, buttons or real relays. The host
therefore treats a capabilities timeout as normal rather than an error.

Unsolicited frames the app sends on startup, one per user serial port —
independent confirmation that the EA1 has exactly two:

```
10 02 a2 00 00 00 02 00 00 5c     UART_RECEIVE port 0
10 02 a2 01 00 00 02 01 00 5a     UART_RECEIVE port 1
```

Control4's `ioserver` logs the matching pair ("Create 3-wire IO Chip serial
port" 1 and 2, on sockets 5101/5102).

## IR output — CONFIRMED

`IROUT_SEND` is opcode `0x66`, answered by `0x68 IROUT_STATUS`. The payload is a
**6-byte header followed by the raw Pronto/CCF words verbatim, big-endian**:

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | u8 | `repeat_count` | `0xFF` = repeat until `IROUT_STOP_RAMP` (0x69). Stock ioserver hardcodes `0xFF`. |
| 1..3 | u24 BE | `output_mask` | bit *N* = IR output *N*; byte 3 holds the low 8 outputs. Not an index — a mask. |
| 4..5 | u16 BE | `code_id` | caller-assigned handle; MCU stores it in a per-output table |
| 6..7 | u16 BE | Pronto word 0 (type) | always `0x0000`; **the MCU never reads it** |
| 8..9 | u16 BE | Pronto word 1 (carrier) | divisor, **not Hz**: `Hz = 4145146 / value` |
| 10..11 | u16 BE | Pronto word 2 | intro/"once" burst-**pair** count |
| 12..13 | u16 BE | Pronto word 3 | repeat-sequence burst-**pair** count |
| 14+2*i | u16 BE | burst words | Pronto carrier-period counts, alternating mark/space |

So ioserver does essentially no conversion: it validates the Pronto string and
passes the words straight through. Burst durations are **carrier periods, not
microseconds**, and element count is derived from the frame length.

Verified on a live EA1 — this exact frame emitted 38 kHz IR on output 1:

```
TX 10 02 66 11 00 00 16 01 00 00 01 00 01 00 00 00 6d 00 02 00 00 01 57 00 ac 00 16 00 16 d1
   (repeat=1, mask=output 1, code_id=1, Pronto 0000 006D 0002 0000 0157 00AC 0016 0016)
RX 10 02 68 11 02 ... 00     IROUT_STATUS, seq echoes 0x11, flags=2 (response), status 0
RX 10 02 68 02 00 ... 04     async follow-up, status 4 — meaning not yet pinned down
```

How it was established: five independent analyses (ioserver's Pronto packer,
ioserver's `firmware_message` serialiser, the Tiva `0x66` handler in Thumb-2, the
HC800 LM3S1162 firmware as cross-reference, and a documentation sweep), each
attacked by three adversarial verifiers. The sender-side and receiver-side
analyses agreeing independently is what makes the layout trustworthy; the
documentation sweep found nothing and its guesses were correctly refuted.

Still open: the semantics of the async `0x68` status value; whether bit 14
(`0x4000`) in a burst word is a long-duration escape (both firmwares appear to
treat values `> 0x3FFF` specially, so raw Pronto words that large may need
splitting); and what payload bytes 6..7 were originally for.

What static analysis of `ioserver` does establish about the layer above it
(director sends IR as a **Pronto/CCF hex string**, which ioserver parses and
repacks):

* only Pronto code type `0000` (raw/learned) is accepted — *"Ir Code Types other
  than 0000 are not supported yet"*
* carrier frequency bounds: **min 20,000 Hz** (`0x4E20`), **max 459,995 Hz**
  (`0x7071B`); 0 is rejected outright
* **max 256 burst pairs** (`0x100`)
* the reported burst-pair count is cross-checked against the parsed count
* per-output validation: *"Output (%d) is not a valid IR output."*

## IR input — WORKING (front receiver, on the MCU)

The EA1 has a front IR receiver, separate from the front-panel blaster and the
four output jacks, and it **is** wired to the Tiva. Capture works.

### The enable sequence (order matters)

```
1. IRIN_SET_CAPTURE_TO_INIT  0x77, zero-length payload
2. IR_PIN_STATE_SET          0x11, 6-byte payload: [0]=0, [1]=0, [2]=pin mask, [3..5]=0
3. IR_MODE_SET               0x41, 1 byte, (mode & 0x30) in {0x30, 0x20}
```

`0x77` is **not** an arm — despite the name it *resets* capture state, and among
other things it zeroes the input pin mask, so on its own it enables nothing.
`0x11` sets that mask: its handler at `0x6912` writes payload[0..5] to
`+0x42c/+0x42d/+0x42e/+0x428/+0x429/+0x42a`, and `+0x42e` (payload[2]) is tested
bit-by-bit as a per-pin mask (`cbz r5` / `lsls r0, r5, #0x1f`). payload[0] must
stay **0**: the capture engine at `0x7b66` only runs its timing math while
`+0x42c == 0`.

> **Never re-arm mid-listen.** Another `0x77` wipes the pin mask and capture goes
> silent. An earlier version of `ohc-ioprobe.py` re-armed every 3 seconds, which
> disabled capture continuously and produced five consecutive false negatives.

### IRIN_CAPTURED (0x97) payload

```
u16 BE   carrier period, in MCU timer ticks
u16 BE   burst words, repeated:
             bit 15 set = carrier ON (mark), clear = space
             bits 0..13 = duration in CARRIER PERIODS
```

Long codes are **split across several frames**, each repeating the 2-byte
carrier header, with the sequence number incrementing. Observed sizes: 202 bytes
(101 words) for full frames, then a short tail frame.

Verified against a real NEC remote — 40 frames, all checksums good:

```
0515 | 0001 8156 00aa 8016 0016 8014 003f 8016 0016 ...
^^^^   carrier period = 1301 ticks
       M342  S170  M22  S22  M20  S63 ...
```

`342 periods / 9 ms` = **38000 Hz** exactly, and the cells decode as textbook
NEC: 9.0 ms + 4.47 ms leader, then 0.58 ms marks with 0.58 ms / 1.66 ms spaces
(NEC specifies 9.0/4.5, 560 us, 1690 us).

This independently confirms the `IROUT_SEND` unit derivation: **durations are
carrier periods, not microseconds**, big-endian u16, in both directions. Note the
one asymmetry — capture flags marks explicitly with bit 15, whereas `IROUT_SEND`
takes raw Pronto words where mark/space is positional (even index = mark).

Reproduce with:

```bash
tools/ohc-ioprobe.py <ea1-ip>:6639 --ir-capture 90
```

## Why vendor `ioserver` could not be used as an oracle

Driving Control4's own `ioserver` to emit IR (which would log the exact frames,
since `debug_ir_output`/`verbose_debug_ir_out`/`debug_transport` hex-dump
everything) is **not** available: its ports 20000/5100 are mutual-TLS and
require a client certificate signed by Control4's product CA. The device's own
self-signed `/etc/ssl/server.pem` completes the TLS handshake but ioserver still
rejects the session ("Failed to accept new socket").

## Flash layout (confirmed)

```
part: TM4C1231D5PM — 64 KB flash, 24 KB SRAM
      (NOT the 256 KB/32 KB of larger TM4C123 parts; the stock image spans
       0x0000..0xFFFF, and SP 0x2000560C rules out a 12 KB part. libopencm3's
       device DB agrees: tm4c123?d5* ROM=64K RAM=24K.)

0x0000 .. 0x0FFF   bootloader (4 KB). In the app image this region is all 0xFF.
0x1000             app vector table:  initial SP 0x2000560C, reset PC 0x00008635
0x1000 .. 0xFFFF   application  (60 KB usable)
```

Getting the SRAM size wrong is not a warning but a dead board: a stack pointer
above the real top of SRAM hard-faults on the first push, before any code can
report it.

`io_start_address 4096` in `/etc/ioserver_config.conf` is exactly this `0x1000`,
and it is the address `COMMAND_RUN` is given. A flash write of the app starts at
0x1000 and leaves the bootloader intact — which is what makes bad app images
recoverable. The bootloader image has its own valid vector table at 0x0000
(SP `0x200010CC`).

## Peripherals the app programs

From literal-pool base addresses (count = number of references):

| Peripheral | Refs | Almost certainly |
|---|---|---|
| GPIOA/B/C/D/E/F | 235 total | IR pins, contacts, LEDs, board straps |
| TIMER0–TIMER3 | 9 each | IR carrier generation + capture (PWM/edge timing) |
| **UART0, UART5, UART7** | 8 each | host link (UART0) + the two user serial ports |
| UART1/2/3/4/6 | 2–3 each | entries in a base-address table at `0x7E9C..0x7EC4` |
| GPIOx_AHB aliases | 2 each | high-speed GPIO path |

TIMER0–3 with per-timer GPIO clusters is consistent with IR output plus IR input
capture; the carrier is generated in software/PWM rather than by a dedicated
block.

## Facts about the silicon that bit us

Recorded because each of these produced, or would have produced, a firmware that
fails with no diagnostic.

* **TM4C1231D5PM is 64 KB flash / 24 KB SRAM** — not the 256 KB/32 KB of larger
  TM4C123 parts. A linker script with the bigger numbers puts `_estack` past the
  top of real SRAM, and the firmware hard-faults on its first push.
* **This part has NO PWM module.** DC1/DC3/DC5 PWM-presence bits read 0. The IR
  carrier must come from a general-purpose timer's CCP pin in PWM mode.
* **GPIO AHB apertures are unmapped at reset.** They stay unmapped until the
  matching bit is set in `SYSCTL_GPIOHBCTL` (0x400FE06C). A UART muxed through
  `0x4005_8000+` without that simply never comes up — reads return zero, writes
  vanish, nothing errors. Use the APB apertures (`0x4000_4000`+) unless there is
  a measured reason not to; the stock application uses APB 238 times to AHB 12.
* **The bootloader does not set VTOR.** The 4 KB TI bootloader contains no
  reference to `0xE000ED08`, so an application at 0x1000 must relocate its own
  vector table. The stock application does exactly this.
* **A complete vector table is 123 entries** (16 system + IRQ 0..106). A short
  table does not fault — peripheral interrupts just vector into whatever code
  follows it in flash.
* **`RCC2.SYSDIV2` is 7 bits when `DIV400` is set** — `{SYSDIV2[28:23],
  SYSDIV2LSB[22]}` = RCC2[28:22]. Masking 6 bits leaves bit 28 stale, which
  works from reset only because RCC2's reset value happens to have it clear.
* **Burst timing needs a 32-bit timer.** Worst case is `16383 carrier periods x
  2500 ticks` (a max-length burst at the 20 kHz minimum carrier) = 40,957,500
  ticks = 819 ms, which fits neither a 16-bit GPTM nor a 16+8 prescaled one.
* **Two errata shape the design.** GPTM#11: in Input Edge-Time count-up mode the
  prescaler misbehaves if `GPTMTnILR` is loaded with 0xFFFF — never write that
  value on the capture path. GPTM#10: writing `GPTMTnMATCHR`/`GPTMTnPR` on an
  enabled timer perturbs the counter in RTC and edge-count modes, with no
  workaround.
* **Package is unconfirmed: PM vs PZ.** Their pin maps differ — PZ has no
  `PB6_T0CCP0` at all. Settle it by reading `SYSCTL_DID1` (0x400FE004) and
  masking `PRTNO` (0x00FF0000): 0x19 = PM, 0x36 = PZ. Do this before trusting
  any pin assignment.

## IR channel pin map — extracted from the stock firmware

The vendor image carries a 10-entry descriptor table at flash **0x2490**, stride
**0x1c**:

```
+0x00  u32  GPIO port base (APB)
+0x04  u32  0x100 | pin_mask
+0x08  u32  timer base
+0x0c  u32  exception number for timer A   (= IRQ + 16)
+0x10  u32  exception number for timer B, or 0xFF
```

| ch | pin | timer | CCP | IRQ A |
|---|---|---|---|---|
| 0 | PD6 | WTIMER5 | WT5CCP0 | 104 |
| 1 | PD4 | WTIMER4 | WT4CCP0 | 102 |
| 2 | PB6 | TIMER0  | T0CCP0  | 19 |
| 3 | PF2 | TIMER1  | T1CCP0  | 21 |
| 4 | PB0 | TIMER2  | T2CCP0  | 23 |
| 5 | PB2 | TIMER3  | T3CCP0  | 35 |
| 6 | PC4 | WTIMER0 | WT0CCP0 | 94 |
| 7 | PC6 | WTIMER1 | WT1CCP0 | 96 |
| 8 | PD0 | WTIMER2 | WT2CCP0 | 98 |
| 9 | PD2 | WTIMER3 | WT3CCP0 | 100 |

**All ten land exactly on the documented CCP0 pin for their own timer**, which is
what confirms the decoding — a 10/10 correlation is not coincidence. The
exception numbers corroborate it independently (35 → IRQ 19 = TIMER0A, 110 → IRQ
94 = WTIMER0A, and so on for all ten). The same `0x100 | mask` encoding appears
in the HC800's LM3S1162 image.

Two things this settles:

* **The package is PM.** Channel 2 uses PB6 = T0CCP0, which exists on
  TM4C1231D5PM and not on the PZ variant.
* **The carrier must come from a GPTM CCP output**, consistent with this part
  having no PWM module at all.

Ten channels because the image is shared across EA1/EA3/EA5. The EA1 populates
fewer — four rear jacks plus a front blaster. **Which physical jack is which
channel is still unknown**; it is a PCB fact, resolvable by driving one channel
at a time and watching which emitter lights.

For reference, the HC800's LM3S1162 uses only TIMER0–3 (four IR channels, no
wide timers) and all three UARTs — UART0 host, UART1/UART2 the two user ports.
