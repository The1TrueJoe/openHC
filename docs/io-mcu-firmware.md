# IO MCU — protocol, bring-up, and firmware notes

The EA1's and EA3's IO processor is a **TI Tiva TM4C1231D5**, running the
**same firmware image on both**. It drives the IR jacks and the combo IR/serial
ports — two of them on EA1, three on EA3 — and is reached over the host UART.
Which peripherals exist on a given board comes from a
[per-board profile table](#the-per-board-profile-table) inside the image.

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
| `0x74` | `0x75` | `00 00 00 00` | CONTACT_GET (u32 bitmask — see below) |
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

## The per-board profile table

One image serves ea1, ea3 and ea5 — `.flash.config` maps all three to the same
`.bin`. It does that with a table of **six board profiles at file offset
`0x1fec`, stride `0x4a4`**, selected at runtime. This table has now been fully
decoded, and it is the authoritative source for per-board IR and serial counts.

Block layout:

```
+0x000  0x18   IR receiver descriptor
+0x018  0x1c   IR output, channel 0
+0x034  0x1c   IR outputs, channels 1..8   (8 x 0x1c)
...
+0x3b4         UART records, 0x28 each, first is the host link
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
+0x08  u32  PCTL value, RX
+0x0c  u32  PCTL value, TX
+0x10  u32  exception number  (= IRQ + 16)
+0x14  u32  default baud      (0x0001c200 = 115200 on every board)
+0x18  u32  GPIO base, RX     +0x1c  u32  RX pin mask
+0x20  u32  GPIO base, TX     +0x24  u32  TX pin mask
```

### Bit 8 is the key

Decoding bit 8 of the pin-mask word as *populated* is what makes the table
resolve. Without it the six blocks look like arbitrary reorderings; with it they
become board profiles, and the counts match physical hardware exactly:

| block | IR outputs populated | user UARTs | board |
|---|---|---|---|
| 0 | 9 | 2 | EA5 / TR1 / amp1 ? |
| 1 | 9 | 2 | EA5 / TR1 / amp1 ? |
| **2** | **5** | **2** | **EA1** — 4 rear jacks + 1 internal blaster |
| **3** | **7** | **3** | **EA3** — 6 rear jacks + 1 internal blaster |
| 4 | 9 | 2 | EA5 / TR1 / amp1 ? |
| 5 | 9 | 2 | EA5 / TR1 / amp1 ? |

Two independent facts confirm the EA3 assignment: the owner counts **6 IR jacks,
with 1–3 doubling as serial**, and a live EA3's `ioserver` opens **three**
user-serial sockets (5101/5102/5103) where an EA1 opens two.

### The decoded profiles

Channels are listed in `IROUT_SEND` `output_mask` bit order. The receiver is
`PD6 / WTIMER5A`, identical in all six blocks.

```
ch  pin  timer  irq    EA1 (block 2)   EA3 (block 3)
0   PD4  WT4    102    populated       populated
1   PB6  T0      19    populated       populated
2   PF2  T1      21    populated       populated
3   PB0  T2      23    populated       populated
4   PB2  T3      35    populated       populated
5   PD0  WT2     98    absent          populated
6   PD2  WT3    100    absent          populated
7   PC4  WT0     94    absent          absent   (PC4 is UART4 RX on EA3)
8   PC6  WT1     96    absent          absent
```

```
UART   pins        irq   EA1 (block 2)   EA3 (block 3)
UART0  PA0/PA1       5   host link       host link
UART5  PE4/PE5      61   user port 1     user port 1
UART7  PE0/PE1      63   user port 2     user port 2
UART4  PC4/PC5      60   —               user port 3
```

Both boards populate a **contiguous run from channel 0**, so `output_mask` is
dense: `0x1f` on EA1, `0x7f` on EA3.

### Relays and contacts fall out of the same table

Between the IR outputs and the UART records each block carries a run of plain
`[gpio_base, pin_mask|attr]` pairs, and bit 8 means the same thing there. Two
groups of four sit at the front:

```
group        pins                    EA1 (block 2)   EA3 (block 3)   blocks 0,1,4,5
first four   PF0 PF1 PF3 PF4         none            PF0 only        all four
second four  PA2 PA3 PA4 PA5         none            PA2 only        all four
then         PC7 PE2 PE3 PB1 PB3     populated       populated       populated
EA3 extras   PA7, PB5                —               populated       —
```

This reproduces the known hardware exactly and is strong confirmation that the
block→board assignment is right:

* **EA1 populates none of the eight** — and an EA1 has no relays and no
  contacts.
* **EA3 populates exactly one from each group** — and an EA3 has **1 relay and
  1 contact** (owner-confirmed off the PCB).
* The 9-output blocks populate all eight, i.e. **4 relays + 4 contacts**, which
  is the HC800/HC250 complement.

**Which group is relays and which is contacts is not established.** Both are
four-wide and nothing in the table distinguishes an output from an input. `PF0`
and `PA2` are each first in their group, so "relay 0" and "contact 0" is the
natural reading, but it should be checked against `ioserver` before being
trusted. The remaining always-populated pins (`PC7`, `PE2`, `PE3`, `PB1`, `PB3`)
are unidentified — LEDs, straps or the front button are all plausible. Note
`PB4` is absent from every block, consistent with it being the ADC board-id
input rather than a GPIO.

### Two corrections this forces to the earlier reading

1. **The `+0x18` descriptor is channel 0, not channel 8.** `include/ir_pins.h`
   already had it that way in its `IR_CHANNELS[]` table; the prose comment above
   the table called it "index 8" and that comment was wrong. Channel 0 is what
   makes both boards' populated sets contiguous.

2. **The channel order is not an unresolved ambiguity.** The earlier note said
   channels 4..7 "come in two different orders … must be measured". They do
   differ between blocks {0,1,5} and {2,3,4}, but that is not a mystery to
   resolve empirically — it is simply which pins each board populates in the
   tail slots, and bit 8 says which those are. For **EA1** the question is moot:
   its five channels are the unambiguous prefix. For **EA3** the two extra
   channels are `PD0/WT2` and `PD2/WT3`, in that order, explicitly.

   What is *still* unmeasured is only **which rear jack corresponds to which
   channel index** — nothing in the image ties a descriptor to a labelled jack.

### The selector, disassembled

The vendor picks its block from an analogue board-ID strap. That function has
now been pulled apart — it lives at runtime address **`0x2878`** (file offset
`0x1878`), and its literal pool sits immediately before the profile table, which
is how it was found: `0x40038000` (ADC0 base) at file `0x1fe0`, twelve bytes
ahead of the table at `0x1fec`.

```
0x2878  push {r4,r5,lr};  sub sp,#4
0x287e  str  r0,[sp]                    ; scratch = 0
0x2882  ldr  r5,[pc,#0x75c]             ; r5 = ADC0 base (0x40038000)
loop:
0x288e  bl   ...                        ; ADCSequenceConfigure(ADC0, 0, ...)
0x2896  bl   ...                        ; ADCHardwareOversampleConfigure(ADC0, 8)
0x289a  movs r3,#0x6a
0x28a2  bl   ...                        ; ADCSequenceStepConfigure(..., 0x6a)
                                        ;   0x6a = CH10 | IE | END  -> AIN10 (PB4)
0x28aa  bl   ...                        ; ADCSequenceEnable
0x28b6  bl   ...; cmp r0,#0; beq 0x28b6 ; spin until conversion done
0x28cc  adds r4,r4,#1                   ; count++
0x28ce  add  r2,sp,#0
0x28d4  bl   ...                        ; ADCSequenceDataGet(ADC0, 0, &scratch)
0x28dc  cmp  r4,#5;  blt loop           ; five conversions
0x28e0  ldr  r0,[sp]                    ; ... and only the LAST one is used
0x28e4  movs r1,#0xfa                   ; 250
0x28e6  udiv r1,r0,r1                   ; id  = adc / 250
0x28ee  mls  r1,r3,r1,r2                ; rem = adc - 250*id
0x28f4  cmp  r1,#0x7e                   ; 126
0x28f8  addge r0,r0,#1                  ; round to nearest
0x28fa  uxtb r0,r0                      ; -> board id
```

Two corrections to the earlier reading of this ("five samples averaged"):

* **The five conversions are a settle-and-discard loop, not an average.** The
  scratch word is *overwritten* by every `ADCSequenceDataGet`; nothing sums it,
  and the `str r0,[sp]` at entry only zero-initialises it. Only the fifth
  reading reaches the arithmetic.
* **The averaging is in hardware** — `ADCHardwareOversampleConfigure(ADC0, 8)`
  makes each returned sample an 8× average.

The id returned is used directly as the block index; 21 separate sites in the
image multiply it by the `0x4a4` stride.

So the strap mapping is fully determined, on a 12-bit ADC against 3.3 V:

| board id / block | ADC counts | volts on PB4 | board |
|---|---|---|---|
| 2 | ~500 | ~0.40 V | EA1 |
| 3 | ~750 | ~0.60 V | EA3 |
| N | N × 250 | N × 0.20 V | — |

**Those voltages are predictions from the decoded arithmetic, not
measurements.** Nobody has put a meter on PB4. Confirm them on a live EA1 and
EA3 and our firmware can drop its compile-time `BOARD=` switch and ship a single
image for both boards, the way the vendor does. Until then it selects at compile
time; the constants are already in `board_profile.h`.

## Contacts — CONFIRMED on an EA3

`CONTACT_GET`'s payload is a **32-bit bitmask, one bit per contact, bit N =
contact N, and a CLOSED contact reads 1**. Verified on a live EA3 by shorting
its single contact input and watching `ioserver` at DEBUG
(`debug_contact_relay 1`):

```
                       jumper OUT   set_state   - Contact State : New State (0x00000000)
                       jumper IN    set_state   - Contact State : New State (0x00000001)
                       jumper OUT   update_state - Contact State :
                                      Current state: (0x00000001) New State (0x00000000)
                                    update_state - Contact State :
                                      Send update of state for contact (0), now (open)
```

Both directions, with an independent fresh read after each change, so this is a
measured edge rather than a single snapshot. The EA3 populates exactly one
contact and it is **index 0**, consistent with the profile table (it populates
one pin from each of the two four-pin groups).

### Contact state is PULLED, never pushed

This is the part that matters for `iod`. The MCU does not volunteer contact
changes, and `ioserver` does not poll on a timer of its own — it re-reads only
when director asks for the `c4.hc.cs` MIB:

```
DEBUG: mib_received - Received MIB: c4.hc.cs
DEBUG: update_state - Contact State : Current state: (...) New State (...)
DEBUG: mib_received - Received MIB: c4.hc.rs      <- relay state, polled alongside
```

In a 45-second idle window with the contact held closed, **not one** contact
line appeared; the observed director polls were roughly 1–2 minutes apart. So a
replacement daemon must run its own `CONTACT_GET` poll loop and derive edges by
comparison — waiting for an unsolicited frame will simply never fire.

Related MIBs seen on the same path: `c4.hc.cs` (contact state), `c4.hc.rs`
(relay state), `c4.hc.fwv` (firmware version, answered `1.0.36`).

### Still not bound to a pin

This tells us the contact is **index 0** and how to read it. It does **not**
say whether index 0 lives on `PA2` or `PF0` — the two candidate pins the EA3
populates. That binding still falls out at first flash: drive the relay pin and
listen for the click, and swap the two constants if it is silent.

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

**Both halves of that sentence are now contradicted by a live HC-800**
([hc800-recon.md](hc800-recon.md)), and neither conflict is resolved:

* **IR count.** The owner of the recon unit counts **six IR jacks** on the rear
  panel, not four. A timer count is a *lower bound* on channel count, not the
  channel count: every Stellaris GPTM has two capture/compare outputs (CCP0 and
  CCP1), so TIMER0–3 can drive up to eight IR carriers. The EA decoding above
  reads channels off the pin/exception table rather than off timer numbers,
  which is why it is trustworthy; the HC800 figure was inferred from timers
  alone. Redo it against the pin table before believing either number.
* **The two user serial ports.** On the live unit `ioserver` holds
  `/dev/ttyS1` and `/dev/ttyS2` — **host 8250 UARTs**, not MCU-routed — at the
  same time as it holds `/dev/ttyS3` for the MCU itself. If the LM3S image
  really does configure UART1/UART2, then either they land somewhere other than
  the rear jacks, or the host UARTs are bridged through the MCU rather than
  wired to the transceivers directly. Nothing visible from a running system
  distinguishes those, and it matters for `iod`: the EA family needs
  `--no-serial-devices`, and whether the HC-800 does too depends on this answer.
