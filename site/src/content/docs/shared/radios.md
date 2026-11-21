---
title: Zigbee, Z-Wave and Bluetooth
description: The box's radios on MQTT, through zigbee-bridge, zwave-bridge and bluetooth-bridge.
sidebar:
  order: 8
---

Each radio has a bridge that runs its network on the box and puts the devices on
the box's MQTT broker, under the same tree as the rest of the box:

| Radio | Bridge | Device | Topics | Network keys |
|---|---|---|---|---|
| Zigbee (the EM357 on every controller) | [zigbee-bridge](https://pulzar-net.github.io/zigbee-bridge/) | `/dev/zigbee` | `<base>/zigbee/...` | `/data/ohc/zigbee` |
| Z-Wave (the CA-1's ZM5304, or a USB stick) | [zwave-bridge](https://pulzar-net.github.io/zwave-bridge/) | `/dev/zwave` | `<base>/zwave/...` | `/data/ohc/zwave` |
| Bluetooth LE (the CA-1's RTL8723BS, or a USB dongle) | [bluetooth-bridge](https://pulzar-net.github.io/bluetooth-bridge/) | `hci0` | `<base>/bluetooth/...` | `/data/ohc/bluetooth` |

`<base>` is the box's topic root, `openhc/<hostname>` unless iod's MQTT settings
rename it (`ohc-mqtt-base` prints it). The bridges' own docs cover the topics:
`bridge/info`, `bridge/request/permit_join`, a device's state and `/set`. The
bridges are upstream projects, built at the revision pinned in their feature's
`packages` file (`board/common/features/zigbee`, `zwave`, `bluetooth`).

## Zigbee

`S91ohczigbee` brings the NCP up first. It runs the app out of the bootloader and
moves it onto the bundled EmberZNet 6.7.8 when it is older
([the radio firmware](/shared/secondary-firmware/)). Then `S92ohczigbee-bridge`
starts the bridge on the held port. The bridge needs EZSP 8 or newer. An NCP on
Control4's 4.7.2 (Control4 Zigbee mode) is left to whatever drives it.

A new box forms its own network on its first start. If the NCP already runs a
network the bridge did not make, the bridge refuses it rather than wipe it. Its
log says which network it found. To keep that network, write its settings to
`/data/ohc/zigbee/network.json`, or restore its backup
([backup and restore](https://pulzar-net.github.io/zigbee-bridge/backup.html)).

`OHC_ZIGBEE_STACK` says who owns the radio. Set it in `board.env`, or in
`/data/ohc/zigbee.env` for one box:

- `bridge` (default): zigbee-bridge, as above.
- `radio`: no bridge. The port is served over TCP (`OHC_ZIGBEE_TCP_PORT`) and
  advertised over mDNS for a zigpy host elsewhere.

## Z-Wave

`S92ohczwave` runs the bridge whenever `/dev/zwave` exists. A stick plugged in
after boot is picked up within 30 s. The device is the board's onboard module
(tagged by its udev rule) or a USB stick with the Silicon Labs Serial API id
`0658:0200` (Aeotec Z-Stick, UZB, Zooz 500 series). Sticks behind a generic
CP210x or CH34x bridge cannot be told from a Zigbee stick or an RS-485 adapter
by their id, so they need a board udev rule that tags them `OHC_RADIO=zwave`.

The controller must run Silicon Labs' Serial API firmware. A CA-1 that was never
set up in Control4 has a blank ZM5304. See [the CA-1 page](/ca1/#radios) for
programming it with Control4's own updater.

## Bluetooth

`S92ohcbluetooth` starts bluetooth-bridge once the box has a controller (`hci0`).
The bridge is its own Bluetooth LE host over the kernel's HCI user channel, with
no BlueZ: it scans, decodes beacons and sensors (BTHome, Ruuvi, iBeacon,
Eddystone) to `<base>/bluetooth/device/<ADDRESS>`, and takes connections, GATT
and pairing as `bridge/request/...`. The kernel only brings the controller up:
the CA-1's onboard RTL8723BS is Realtek H5 on UART2 (`hci_uart` loads its
firmware), and a USB dongle is `btusb`.
