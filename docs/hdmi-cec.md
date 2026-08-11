# HDMI CEC

The CE5310 integrates HDMI 1.4a, and 1.4a includes CEC. So the question is not
whether the silicon can do it — it is whether anything between the pin and
userspace is wired up. On the EA1 the answer is no, at every layer, and the one
place CEC does appear is somewhere nobody would guess.

## Nothing on the HDMI path

| Where we looked | What we found |
|---|---|
| `/dev` | no `cec` node. `gdl`, `dri`, `avcap_core`, `p_unit`, `sec`, `i2c-0..3` are all there |
| `/proc/devices` | no cec char major |
| `/proc/modules`, 286 `.ko` under `/lib/modules` | no cec module |
| `/proc/config.gz` | no cec symbol — the kernel is 3.12, and the mainline CEC framework landed in 4.8 |
| `libgdl.so` (host and container copies) | no cec symbols, no cec strings |
| `pd_hdmi.ko`, `gdl_server.ko` | nothing (377 `hdmi` strings in `pd_hdmi`, so the search was working) |
| CEFDK source + all 79 kernel patches in the GPL drop | zero matches for `cec` |
| Android container rootfs | no CEC HAL, no `*hdmi*` files at all |

The decisive one is the port driver's own attribute table:

```
portattrs -port HDMI -dump
```

42 attributes, every single one TMDS, HDCP, DDC or EDID — `cable status`,
`use edid`, `slow ddc`, `ddc speed`, `edid_read retry`, `I2C bus reset`,
`power`. There is no CEC attribute. Whatever Intel wired into this port driver,
CEC was not part of it.

## The TV is ready even if we are not

Reading the sink's EDID settles what the other end can do:

```
PNP MTC, monitor name "100012589", EDID 1.3, 71x40 cm
native 1920x1080p60, preferred DTD 148.50 MHz
CEA-861 rev 3: 10 SVDs, 4 SADs (LPCM 2ch, AC-3 6ch, E-AC-3 8ch, MLP 8ch)
HDMI VSDB: 66 03 0c 00 10 00 80
```

That VSDB is the interesting part. IEEE OUI `00-0c-03` is the HDMI Licensing
identifier, and the two bytes after it are the **Source Physical Address**:
`10 00` = 1.0.0.0. The TV has already assigned this box a CEC address on its
tree. The sink is holding up its end of the protocol and waiting for a device
that never speaks.

### Why reading the EDID used to hang

Worth writing down, because it burned two ten-minute timeouts before anyone
looked properly. `gdl_port_recv` is declared as

```c
gdl_ret_t gdl_port_recv(uint32_t port, uint32_t recv_id, void *buf, uint32_t size);
```

The fourth argument is a size **by value**. Passing a `uint32_t*` there — which
is what the first attempt did, on the assumption it was an in/out length — makes
the call never return. The other half of it: the buffer is 129 bytes and
bidirectional. Byte 0 is the block index on the way in, bytes 1..128 are the
block on the way back. Port id is 2 (`GDL_PD_ID_HDMI`) and recv id is 2; neither
needs discovering.

All of that came out of disassembling the vendor's own `hdmi_edid_dump` sample,
where the call is plainly `gdl_port_recv(2, 2, buf, 0x81)`. Enumerating recv ids
by string name, which is what the broken version did, was wasted effort.

## The one place CEC does exist: the PIC UART

`/dev/ttyS2` goes to a Microchip PIC24 — a discrete part on the board, not the
8051 inside the SoC that CEFDK reports at boot. Intel's own diagnostic tool is
what settles that, and it also confirms the port assignment is Intel's, not
Control4's:

```
$ strings /usr/dtsbin/pic24
The uart device is wrong, use '-dev /dev/ttyS2' parameters for CE4200 and CE5300 and CE2600
InitPIC24()
PIC24 Version:%s
```

`intel_pic_uart.ko` and its userspace half `/lib/libpicuart.so` are likewise
Intel's, not Control4's — the banner reads `#@# libpicuart.so 36.0.14495.347773`,
r36-CEFDK vintage. Both carry a full CEC message class:

```
LR_PICInterface::sendCECAck()
PicBufferOutgoingCEC::{setHeader,setCommand,setParameterLength,serialize}
PicBufferIncomingCEC::{serialize,unserialize}
PicBufferCECAck::{isAck,getOpcode,serialize,unserialize}
```

The library ships DWARF, so the layouts come out directly:

```
PicBuffer            +0x04 cmd; +0x05 length
PicBufferOutgoingCEC +0x06 CecBroadcast; +0x07 CecParameters[14]; +0x15 CecLength;
                     +0x16 CecParameterLength; +0x17 CecHeader; +0x18 CecCommand
PicBufferIncomingCEC +0x06 CecCmd; +0x07 CecHeader; +0x08 CecLength;
                     +0x09 CecParameterLength; +0x0a CecParameters[16]
PicBufferCECAck      +0x06 CecCmd; +0x07 CecAck
```

Header nibbles, an opcode, up to fourteen operands and a broadcast flag. That is
a CEC frame, not something that resembles one.

The wire framing falls out of the `.data` constants and the disassembly of
`sendCECAck`:

```
AA <n> <body as ASCII hex> <XOR checksum as ASCII hex>      n = hex chars in body

ack       AA 02 "0606"      body 06
nak       AA 02 "0707"      body 07
CEC ack   AA 04 "050005"    body 05 00
```

So **PIC command 0x05 is CEC**. Note the field order in memory is not the wire
order, and `serialize()` is not compiled into this build — the outgoing path
exists in headers and debug info only. Whoever wants to transmit gets to work it
out from the incoming direction.

## Why that does not help

The PIC UART is `/dev/ttyS2`, and `/dev/ttyS2` is the watchdog link. One
process owns it — Control4's `watchdogd` — and it is feeding a hardware
watchdog that resets the board when the heartbeat stops. A second writer on that
port is a reboot.

There is a zero-risk way to listen, though. `watchdogd` logs every message the
PIC sends it, and logs `Unknown cmd(N)` for anything it does not handle. Over
ten and a half minutes with a powered, CEC-capable TV attached: 66 messages, all
`cmd(26)` heartbeats at one per ten seconds. No `cmd(5)`. No unknown commands.

The PIC has never forwarded a CEC message. That is suggestive rather than
conclusive — CEC-active sinks tend to poll on topology change rather than
continuously — but combined with the total absence of CEC anywhere else, it
points one way.

Two facts stay genuinely unverified, and they are the two that matter: whether
HDMI pin 13 is physically routed to the PIC on this board at all, and whether
Control4's PIC firmware implements command 0x05. Neither can be settled from
software. There is no PIC24 image anywhere on the box either — `/control4/firmware/`
holds images for *peripherals* (AT128 amps and switches, LM3S and TM4C1294
dimmers, keypads, RS-485 gateways), not for the controller's own
microcontrollers — so the PIC firmware is not field-updatable here.

## Ways this could actually work

Cheapest first.

**A USB CEC adapter.** `CONFIG_USB_ACM=y` is built into the kernel and char
major 166 is registered, so a Pulse-Eight style adapter enumerates with no
kernel work at all. libcec talks to it over the resulting `/dev/ttyACM0`. This
is the only option that does not depend on an unverified hardware fact.

**IR instead.** The TM4C already drives five IR outputs and the code library and
transmit path are working. This is also, in practice, how Control4 turns
televisions on. Not CEC, but it solves the actual problem.

**Own the PIC link.** Replace `watchdogd` with a daemon that owns `/dev/ttyS2`
and multiplexes the heartbeat with everything else the PIC can do — CEC,
`setGPIOValue`, `setPWM`, `setIrRepeatMode`. That is the only route to native
CEC on this hardware, it is gated behind both unverified facts above, and
getting the heartbeat wrong reboots the box. It is worth doing for its own
reasons; CEC would be a side effect, not the justification.

## Cost of finding this out

One reboot. A recursive `grep` across `/usr /lib /bin /sbin` ran long enough to
starve `watchdogd`, and the PIC reset the board — the same failure mode as the
CPU benchmark. Ten minutes of sustained filesystem work is apparently enough.
Every `cec` hit that grep produced was a substring false positive: `xterm`,
`apt-cache`, `Xorg`.
