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
control-system integration bug. It is also exactly what MQTT's retained flag
means, which is why the topic map falls out of it for free.

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
| retained | yes | no |
| a late subscriber | is told immediately | has missed it |

Events are opt-in because they are not small: pushing a chatty projector's byte
stream at a page that only wanted contact changes is a waste. State always
flows, because a client holding a stale mirror is actively wrong.

## Reaching it

IO control is **MQTT**, and only MQTT. iod serves its own topics — there is no
broker to install:

* **browser** — `ws://<host>/mqtt`, proxied by webd from the port the page was
  served on. mqtt.js on the other end. One open port for the whole GUI.
* **anything else** — plain MQTT on `:1883`, or point iod at your existing
  broker and subscribe there.

Those are the same topics in all three cases. A relay closed from the config
GUI, from Home Assistant, or from `mosquitto_pub` is the same publication.

REST keeps what MQTT is bad at, on `:7070` (or `/iod/…` through webd):

```
GET  /api/health            never authenticated
GET  /api/io                what the board HAS — needed before you know the topics
GET  /api/io/mcu            part, firmware version, measured baud
GET  /api/config            MQTT settings, minus the password
POST /api/config            change them; the connection restarts, no reboot
```

Configuration lives on REST deliberately: editing your own transport over that
transport is a good way to lose a controller.

## Topics

`<prefix>/<id>/…`, where prefix defaults to `openhc` and id to the hostname.

```
<base>/status              online | offline    retained, last will
<base>/state/relay/0       ON | OFF            retained
<base>/state/contact/0     ON | OFF            retained
<base>/state/mcu/link      ON | OFF            retained
<base>/state/serial/0/baud 115200              retained
<base>/state/serial/0/viewers                  retained
<base>/event/ir/rx         {"pronto":"…"}      NOT retained
<base>/event/serial/0/rx   {"b64":"…"}         NOT retained
<base>/error               {"code":…,"message":…}
<base>/cmd/relay/0/set     <- ON | OFF | TOGGLE
<base>/cmd/ir/0/send       <- pronto hex
<base>/cmd/serial/0/write  <- bytes
<base>/cmd/serial/0/baud   <- 115200
<base>/cmd/raw             <- any command, as JSON
```

Command payloads are the plain strings an automation tool sends, because half
of what publishes here will be a Home Assistant switch or a one-line shell
script. `cmd/raw` takes the full JSON form for everything else:

```json
{"op":"serial.write","index":0,"data":"PWR ON\r"}
```

Subscribing IS the snapshot — state is retained, so a page loaded an hour after
the last change still renders the truth with no separate "give me everything"
round trip.

`relay/N/set` is **idempotent**. The IO microcontroller has no set opcode, only
`TOGGLE` and `GET`, so iod reads first and toggles only on a mismatch. Toggle is
not idempotent: a retained message replayed on a broker reconnect, or an
automation that fires twice, would otherwise leave the relay inverted.

## Serving, bridging, or both

Two independent switches:

* **serve** — carry the topics here. The config GUI needs this; it is on by
  default and the settings page says plainly what turning it off costs.
* **bridge** — also connect out to somebody else's broker.

Independent on purpose. If the GUI depended on the house broker, a broker that
was down or misconfigured would take out the very screen you would use to fix
it.

`mqtts://` uses rustls with the **ring** provider — chosen over rustls's default
because that one needs CMake and a cross C toolchain, and this workspace links
with `rust-lld` precisely so it needs neither. A private or self-signed broker
**must** set a CA path: a minimal firmware image ships no system trust store,
and iod refuses to connect rather than skipping verification.

### Home Assistant

With discovery enabled (the default), the controller appears by itself, relays
as switches and contacts as binary sensors, no YAML by hand. That is the payoff
for retaining state and not retaining events.

## The serial terminal — `ws://host/iod/ws/serial/{n}`

Raw bytes, and the one thing that is deliberately NOT MQTT. A console is a byte
stream with backpressure and scrollback; publishing every keystroke and
base64-ing every chunk would be a worse terminal and no simpler.

iod opens each port once and every viewer shares that session: all of them see
the same stream, any of them can type, and joining replays recent scrollback so
arriving mid-session is not a blank screen. `serial/{n}/viewers` is how the UI
says somebody else is on the same console.

Serial RX is *also* published as an event, so an external system can trigger on
what a device says without holding a terminal open.

## Settings

`/etc/openhc/iod.json`, written by the GUI's settings page. Environment
variables from `/etc/openhc/iod.conf` **override** it and are shown in the UI as
read-only — a fleet provisioned by dropping in a conf file should not find the
GUI silently disagreeing, and an operator should see why their edit will not
take rather than watching it vanish on restart.

## Authentication

Off unless `IOD_TOKEN` is set, because the first thing this has to do is work on
a bench with a serial cable and no configuration. Set it on anything reachable
from a wider network — without it, anything that can route to the box can close
a relay.

The token is used three ways, because it is one secret for the box rather than
one per protocol:

* REST — `Authorization: Bearer <token>`
* WebSockets — `?token=…` on the URL. Not a weaker option by choice: the browser
  WebSocket API cannot set request headers.
* MQTT — as the CONNECT password. A client that gets it wrong is refused with
  `BadUserNamePassword` rather than being quietly ignored.

`/api/health` stays open so a monitor can see the daemon is up without holding
a credential.

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
