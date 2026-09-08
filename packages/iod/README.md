# iod — the openHC IO server

Owns **every** local IO on the controller: the user serial ports, IR out and in,
the relays and the contacts. Nothing else opens those devices — the config GUI
(`webd`) is a *client* of this over `:7070`, the same as any external control
system. That single-owner rule is what keeps one process on a UART that can only
answer one question at a time.

One binary runs the whole fleet. What differs between an HC-800, an EA3, an IO
Extender and a CA-1 is `/opt/ohc/board.env` and nothing else.

## State and events are different things

This distinction runs through the whole API, and getting it wrong is the classic
control-system integration bug.

**State** is what the box currently *is* — relay 2 is closed, contact 1 is made,
the MCU link is up. It has a value at every instant, a new client must be told it
on connect, and a client that missed a change is *wrong* until the next one. So
state is mirrored inside iod, sent as a snapshot to every new subscriber, and
published as a delta whenever it changes — no matter who changed it. Two people
on the config GUI see the same relay.

**Events** are things that *happened* — a byte arrived on a serial port, a remote
was pressed at the IR receiver. They have no value between occurrences,
snapshotting them is meaningless, and a client that connects late has simply
missed them. These stream, and are never retained.

| | State | Event |
|---|---|---|
| examples | `relay/0`, `contact/0`, `mcu/link`, `serial/0/baud`, `serial/0/viewers` | `ir/rx`, `serial/0/rx` |
| on connect | snapshot | nothing |
| duplicates | suppressed — a delta means a real change | never suppressed |
| over MQTT | retained | not retained |
| subscription | always delivered | opt-in |

Events are opt-in because they are not small: pushing a chatty projector's byte
stream at a page that only wanted contact changes is a waste. State always
flows, because a client holding a stale mirror is actively wrong.

## Reaching it

`:7070` directly, or — from a browser — through webd's proxy on the port the
page was served from: `/iod/api/…` and `/iod/ws/…`. The GUI uses the proxy so
it needs exactly ONE port reachable. A page on :80 that fetches :7070 requires
both to be open from wherever the operator is sitting, and a single restrictive
network turns the whole config UI into an error card while the box is fine.

Everything below works identically on either path.

## The control socket — `ws://host:7070/ws/control`

The primary surface. Commands, state and events on one connection. This is what
the GUI's panels use and what an external control system should use.

On connect you are sent the state document:

```json
{"seq":41,"ts":1757370000.5,"type":"snapshot",
 "state":{"relay":{"0":false,"1":true},"contact":{"0":false},
          "mcu":{"link":true},"serial":{"0":{"baud":115200,"viewers":1}}}}
```

Then deltas as things change:

```json
{"seq":42,"ts":1757370012.1,"type":"state","path":"relay/0","value":true}
```

Subscribe to the events you want; filters use MQTT's rules (`#`, a trailing
`/#`, or a bare prefix matching its own subtree):

```json
{"op":"subscribe","topics":["ir/rx","serial/0/rx"]}
```
```json
{"seq":43,"ts":1757370033.9,"type":"event","topic":"ir/rx",
 "data":{"pronto":"0000 006d 0022 0002 …","bytes":68}}
```

Commands carry an `id`, echoed on the reply, so several can be in flight:

```json
{"id":7,"op":"relay.set","index":1,"on":true}
{"id":7,"ok":true,"result":{"index":1,"on":true}}
```
```json
{"id":8,"ok":false,"error":{"code":"bad_request","status":400,
                            "message":"relay 9 does not exist (0..3)"}}
```

`seq` is monotonic across the whole stream. If you fall behind, you are told —
and resent the snapshot — rather than silently losing a relay change:

```json
{"type":"lagged","missed":12}
```

### Commands

| `op` | arguments | notes |
|---|---|---|
| `capabilities` | | what this board has; sections are absent when the count is zero |
| `state.get` | | the mirror, served from memory |
| `mcu.info` | | part, firmware version, measured baud |
| `contact.get` | | forces a read rather than using the mirror |
| `relay.get` | | |
| `relay.set` | `index`, `on` | **idempotent** — see below |
| `relay.toggle` | `index` | |
| `ir.send` | `port`, `pronto`, `repeat` | Pronto type `0000` only; durations are carrier periods |
| `serial.list` | | ports and the bauds iod accepts |
| `serial.baud` | `index`, `baud` | applies to the shared session — every viewer moves together |
| `serial.write` | `index`, `data`, `hex?`, `b64?` | write without holding a terminal open |
| `subscribe` / `unsubscribe` | `topics` | socket-level; events only |
| `ping` | | |

`relay.set` matters more than it looks. The IO microcontroller has no *set*
opcode, only `TOGGLE` and `GET`, so iod reads first and toggles only on a
mismatch. Toggle is not idempotent: a retained MQTT message replayed when a
broker reconnects, or an automation that fires twice, would otherwise leave the
relay inverted.

## The terminal socket — `ws://host:7070/ws/serial/{index}`

Raw bytes, because a console stream is not JSON. It attaches to the **same
shared session** the control socket reports on: iod opens each port once, and
every viewer sees the same stream and can type into it. Two installers on one
console see each other's keystrokes — which is what makes it a shared console
rather than two people fighting over a cable. Joining replays recent scrollback,
so arriving mid-session does not mean staring at a blank screen.

`serial/{n}/viewers` in the state mirror is how the UI says somebody else is here.

## REST

The same commands, one per request, for scripts and curl. It cannot deliver
events, so anything reactive wants the socket.

```
GET  /api/health                     (never authenticated)
GET  /api/io                         capabilities
GET  /api/state
GET  /api/io/mcu
GET  /api/io/contacts
GET  /api/io/relays
POST /api/io/relays/set              {"index":1,"on":true}
POST /api/io/relays/toggle           {"index":1}
POST /api/io/ir/{port}/send          {"pronto":"0000 …","repeat":1}
GET  /api/io/serial
POST /api/io/serial/{index}/baud     {"baud":9600}
POST /api/io/serial/{index}/write    {"data":"hello\r"}
```

## MQTT

A bridge, not the core — a browser cannot speak raw MQTT and a controller has to
work with no broker on the network. Configure it in `/etc/openhc/iod.conf`.

```
<prefix>/<id>/status              online | offline   retained, last will
<prefix>/<id>/state/relay/0       ON | OFF           retained
<prefix>/<id>/state/contact/0     ON | OFF           retained
<prefix>/<id>/event/ir/rx         {"pronto":"…"}     not retained
<prefix>/<id>/event/serial/0/rx   {"b64":"…"}        not retained
<prefix>/<id>/cmd/relay/0/set     <- ON | OFF | TOGGLE
<prefix>/<id>/cmd/ir/0/send       <- pronto hex
<prefix>/<id>/cmd/serial/0/write  <- bytes
<prefix>/<id>/cmd/serial/0/baud   <- 115200
<prefix>/<id>/cmd/raw             <- any control-socket command, as JSON
```

Command payloads are the plain strings an automation tool sends, because half of
what publishes here will be a Home Assistant switch or a one-line shell script.
`cmd/raw` is the escape hatch for everything else.

`mqtts://` uses rustls with the **ring** provider — chosen over rustls's default
because that one needs CMake and a cross C toolchain, and this workspace links
with `rust-lld` precisely so it needs neither. A private or self-signed broker
**must** set `IOD_MQTT_CA`: a minimal firmware image ships no system trust store,
and iod refuses to connect rather than skipping verification.

### Home Assistant

With discovery enabled (the default), the controller appears by itself, relays as
switches and contacts as binary sensors, no YAML by hand. That is the payoff for
doing the retained/not-retained split properly.

## Authentication

Off unless `IOD_TOKEN` is set, because the first thing this has to do is work on
a bench with a serial cable and no configuration. Set it on anything reachable
from a wider network — without it, anything that can route to the box can close
a relay.

Clients send `Authorization: Bearer <token>`, or `?token=…` on the URL. The
query form is not a weaker option by choice: the browser WebSocket API cannot
set request headers. `/api/health` stays open so a monitor can see the daemon is
up without holding a credential.

## Not done yet

- **MCU-routed serial** (EA combo ports) — those have no device node; their
  bytes travel over the IO protocol's UART opcodes. iod reports the port and
  says the bridge is missing rather than opening a `/dev/tty` that never existed.
- **GPIO backend** (IO Extender contacts) — reports "not implemented" rather
  than all-open, which would be a valid-looking lie.
- **IR capture enable is unverified.** The opcode is the vendor's, but its
  one-byte payload is inferred from the name and the firmware sends no reply, so
  a wrong value fails silently. If a remote press produces no `ir/rx` event, that
  is the first suspect.
