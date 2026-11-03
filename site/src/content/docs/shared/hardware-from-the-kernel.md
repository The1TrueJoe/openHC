---
title: Hardware from the kernel
description: Where every daemon and init script reads a board's hardware from, what a board must provide for each, and what board.env still holds.
sidebar:
  order: 0
---

A board's hardware is described once, in its device tree (the HC-800 adds ACPI
tables, `board/hc800/acpi/`), and read back from what the kernel registered.
Where the kernel cannot name a thing (which tty the Zigbee radio is on, what the
rear panel calls an HDA codec's PCM), the board says so in one udev rules file,
`etc/udev/rules.d/60-openhc-board.rules` in its overlay. `board.env` holds
software defaults only.

## Where each fact comes from

| Fact | Source | Read by |
|---|---|---|
| Model | DT `model` (`/proc/device-tree/model`): `Control4 EA-3 v1`, `Control4 HC-800` | iod, webd (`/api/board`), audiod (default endpoint names), `ohc-mdns`, the banners |
| Board name | the first `control4,<board>` DT `compatible`; the build's `/etc/openhc-release` `board=` | `ohc-hw board`, the flasher |
| Managed-switch ports | the DSA user ports the kernel created (`DEVTYPE=dsa`), named by their DT `label` | switchd, `S39ea-switch`, `S40net`, `S87ohcswitch` |
| mDNS interface | every interface that is up, minus loopback, DSA ports and conduits and bridge ports; wired first | `ohc-mdns` |
| IO backend | a serdev device whose `modalias` has `control4,io-mcu` (DT or ACPI `PRP0001`) means `mcu`; else `relay1..`/`contact1..` lines or `openHC IR out N` lirc devices mean `gpio`; else none | iod |
| IO-MCU part | `/sys/class/ohc/iomcu/part`; with the driver unbound, the compatible before `control4,io-mcu` (`control4,tm4c1231d5-io-mcu`) | iod (`/api/io` `part`) |
| IO counts, combo jacks | `/sys/class/ohc/iomcu`, `/sys/class/tty/ttyOHC*/{jack,label,mode}` | iod; see [the IO-MCU](/shared/io-mcu/) |
| Host serial ports | `/dev/ohc/serial/<n>`, from the board's udev rule | iod |
| Zigbee and Z-Wave radios | `/dev/zigbee`, `/dev/zwave`, from the board's udev rule | zigbee, `S91ohczigbee` |
| Audio outputs and inputs | every ALSA playback and capture PCM (`/proc/asound`) | audiod |
| Output label | the PCM's name: a DT sound card's link name, `HDMI` on the EA's HDMI card; or the board's udev rule | audiod |
| DSP profile | the codec node's `firmware-name`: `openhc/<x>.bin` means `/usr/share/openhc/dsp/<x>.json` | audiod |
| Default Spotify, AirPlay, library names | `<product> <label>` per output (`<product>` alone on a one-output board), library on the first output; renames are saved in `/data/ohc/audio.json` | audiod |

The product is the model without its vendor and variant: `Control4 EA-1 v2 PoE`
gives `EA-1`. An output's id (`analog1`, `coax`, `hdmi`) is its label in lower
case, without spaces or anything in parentheses; saved endpoints refer to it.
Outputs are listed by label with HDMI last.

`ohc-hw` (`/usr/bin/ohc-hw model|board|dsa-ports|lan-ifaces`) gives init
scripts the same answers.

## The board's udev rules

`70-openhc-io.rules` (shared) turns these properties into names:

| Property | On | Gives |
|---|---|---|
| `OHC_SERIAL=<n>` | a tty | `/dev/ohc/serial/<n>` |
| `OHC_SERIAL_LABEL=<text>` | a tty | its label in iod and the UI (default `Serial <n>`) |
| `OHC_SERIAL_MODEM=1` | a tty | DTR and RTS are wired (a DB9): iod holds them up and offers `serial/N/dtr`, `rts` |
| `OHC_RADIO=<kind>` | a tty | `/dev/<kind>` and `/dev/ohc/radio/<kind>` |
| `OHC_AUDIO_LABEL=<text>` | a PCM node (`pcmC*D*p`, `c`) | its label; where any of a card's playback (or capture) PCMs has one, its others in that direction are not used |
| `OHC_AUDIO_SWAP=1` | a PCM node | left and right are exchanged between PCM and jack |

MCU-routed ports (`ttyOHC<n>`) are `/dev/ohc/serial/<n+1>` with no rule. Match a
UART by something that does not move: its platform device on a DT board
(`KERNELS=="2034000.serial"`), its I/O port on the HC-800's ACPI 8250s
(`ATTR{port}=="0x2F8"`), its USB id less the rear port on the EA.

| Board | Rule file says |
|---|---|
| EA family (`board/ea/common`) | the CP2104 (`10c4:ea60`) is `zigbee`, unless it is on the rear USB port (`01:0d.1`) |
| HC-800 | ttyS1 and ttyS2 are `RS-232 port 1`/`2` with modem lines; ttyS4 is `zigbee` (and `ttySZigbee`); the ALC888 PCMs are `Analog 1`, `Analog 2` (swapped), `Coax (S/PDIF)`, `HDMI`, `Line in` |
| CA-1 | UART3 is serial port 1 (`RS-232/485`); UART4 is `zwave`, UART5 `zigbee` |
| IO Extender | the FPGA's four 16550s are `RS-232 port 1`–`4` with modem lines |

## Sound cards

A device-tree sound card names its PCMs through its links. ASoC calls a link's
PCM `<link name> <codec DAI>-<device>`, and audiod drops the codec part, so a
link named `Analog 1` gives an output labelled `Analog 1`. A
`simple-audio-card` link's codec (its `sound-dai`) is where audiod looks for
`firmware-name`; on a card with `dai-link@N` children, link N is PCM device N.

No format hint is needed. audiod's dmix asks for S16_LE and, on hardware that
cannot take it (the CE5300 I2S and HDMI take S32_LE only), takes the
hardware's own format; the `plug` in front converts.

## What board.env still holds

Software defaults, one file per family: `OHC_ID_IFACE`, `OHC_UPLINK_IFACE`,
`OHC_WIFI_IFACE`, `OHC_TIMEZONE`, `OHC_NET_WATCHDOG`, the Zigbee baud, TCP port,
radio type and stock-archive path, the restore button's hold time and LED,
`OHC_AUDIO_RATE`, `OHC_AUDIO_HELPERS`, and the EA's `OHC_KEXEC_CMDLINE_EXTRA`.

The HC-800's file still carries some hardware lines that nothing here has moved
yet: the ICH GPIO offsets (`OHC_GPIO_*`, which iod reads for the IO-MCU reset),
the boot partitions and the video registers.

A board.env that still sets one of the moved variables (`OHC_MODEL`,
`OHC_DSA_PORTS`, `OHC_SERIALS`, `OHC_AUDIO_OUTPUTS`, ...) is ignored for it.
