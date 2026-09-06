---
title: Ethernet and the BCM53125 switch
description: The fake-PHY e1000, the managed switch behind it, and a port map settled by a controlled traffic test.
sidebar:
  order: 7
---

The EA3's networking is the one genuinely new subsystem versus the EA1, and it
is not shaped the way it looks.

```
eth0        e1000 (PCI 01:0c.0), MAC 00:0F:FF:94:EE:02
            dmesg: "GBE working in Internal Fake Phy Mode"
                   "e1000_copper_link_preconfig: Phy ID = 0x3625f20"
switch      spi-bcm53125 on spi0.1
            resets: gb_sw_reset (gpio8), gb_eth_reset (gpio5)
```

**The e1000 MAC doesn't talk to a PHY at all.** It runs in "internal fake phy"
mode against a fixed link, and the **BCM53125 managed switch sits behind it on
SPI, not MDIO**. The two external RJ45s are switch ports; the CPU is another
switch port.

This is why openHC carries an `e1000` fake-PHY patch, and it's applied always —
not as part of the `switch` feature, because it's what gives `eth0` a link on
the primary jack at all.

## The switch is fully inspectable from a running stock unit

The vendor driver exports a raw register window plus per-port counters:

```
/sys/bus/spi/devices/spi0.1/port      rw   port selector (0-5 and 8; >5 clamps to 8)
/sys/bus/spi/devices/spi0.1/page_reg  rw   "0x01 0x00" style page/register selector
/sys/bus/spi/devices/spi0.1/value     r    reads the selected register
/sys/bus/spi/devices/spi0.1/mibs      r    full MIB counter block for the selected port
```

:::danger
Reading is harmless. **Do not write `value`** on a unit you reach over the
network, the path to it runs through this switch.
:::

## The port map, settled by measurement

Reading registers gets you partway:

```
page 0x01 reg 0x00  Link Status Summary = 0x24   -> ports 2 and 5 up
page 0x00 reg 0x5d  port 5 GMII override = 0x4b  -> forced link, 1000M, full duplex
```

But it was settled by a **controlled traffic test** rather than inference:
snapshot all ports, push ~4 MB out of the box, snapshot again.

```
port 5   dRxGood = 4,313,853     <- switch receives it from the SoC MAC
port 2   dTx     = 4,313,811     <- switch sends it out the jack
ports 0,1,3,4    zero
```

A 42-byte discrepancy across 4 MB is framing. That is direct proof, not a
correlation.

| Switch port | Role | Evidence |
|---|---|---|
| **5** | **CPU port** to the SoC e1000 | 4.3 MB delta inbound during the push; forced 1000/full override |
| **2** | primary rear RJ45 | 4.3 MB delta outbound during the push; link up with a partner |
| **1** | second rear RJ45, **isolated** | links and receives, but never forwards |
| 0, 3, 4 | unused | no link, no counters, no delta |
| 8 (IMP) | **not** the CPU port here | no link, all-zero counters |

Ports 0–4 all report the same integrated PHY (`0362:5f24`). They're the
BCM53125's five built-in PHYs, so **silicon presence cannot distinguish a routed
jack from an unrouted one.** Port 1 was identified by moving the cable.

:::caution[Counter caution]
These MIBs are cumulative, not clear-on-read (a port with no link holds its value
across repeated reads), but they **do** reset on reboot, and the recon unit
rebooted mid-investigation. Take a snapshot immediately before and after any
experiment rather than trusting older numbers.
:::

## The second jack is live but not bridged to the CPU

Moving the cable to the other RJ45 makes the unit **completely unreachable**, and
that isn't a dead port. While the cable sat there, port 1's counters told the
whole story:

```
RxOctets / RxGoodOctets : 1,455,777      TxOctets : 0
RxMulticastPkts         : 4,565
RxBroadcastPkts         : 224
RxUnicastPkts           : 23
```

4,812 frames averaging 302 bytes, ordinary LAN flood. So the PHY links, the port
receives, and the switch **transmitted exactly zero bytes out of it** while the
host never answered. The stock switch configuration isolates that jack from the
CPU port.

The vendor driver's register window won't confirm the mechanism, it returns
"Not Supported or Not Implemented" for the VLAN pages (`0x31`, `0x34`), so the
port-based VLAN map can't be read through it.

**This is the argument for the DSA work.** Without a switch driver the second
RJ45 is unusable, and no amount of `e1000` configuration changes that.

## Where the port to mainline stands

The vendor `spi-bcm53125` driver is out-of-tree. Mainline's equivalent is the
**DSA `b53` driver**, which does support the BCM53125 and does have an SPI
binding, so this is a *portable* subsystem — unlike the graphics stack.

It needs a device-tree or board description, which is the same de-DT problem
already solved for `i2c-pxa`. The board glue lives at
`board/ea-common/drivers/spi/spi-ea-b53-board.c` and **now builds**: its old
failure was an include, not a design problem — `dsa_chip_data` moved to
`<linux/platform_data/dsa.h>`, and only the `dsa_platform_data` wrapper was
actually removed.

**DSA's bring-up of the isolated second jack is still unproven on hardware.** If
DSA doesn't attach, the network init script falls back to DHCP on the master
interface and the primary jack works as before, which is the behaviour the
`switch` feature being currently disabled relies on.

The feature is switched off at present to keep an image under the
[copy window](/ea/boot-chain/#the-copy-window). It isn't needed to reach
the network: the always-applied fake-PHY patch gives `eth0` on the primary jack
on its own.
