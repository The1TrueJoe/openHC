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

It needs a board description. That is now a node in the board device tree,
`board/ea/common/kernel/arch/x86/boot/dts/ea-switch.dtsi`: the switch at SSP
chip-select 1 in SPI mode 3, ports 1 and 2 as `lan1`/`lan2`, and port 5 as the
CPU port with `ethernet = <&gbe>` pointing at the e1000. (It used to be C
platform data, which DSA treats as legacy and which silently needed the CPU
port's conduit filled in by hand.) b53 only binds when `S39ea-switch` loads it,
which it does with `ohc.switch=1` on the kernel command line.


**DSA attaches on hardware and carries traffic.** On 2026-10-28, an EA3 booted
through the durable kexec path with `ohc.switch=1` came up with
`found switch: BCM53125, rev 4`, `lan1`/`lan2` and `DSA: tree 0 setup`. switchd's
`br-lan` leased the box's address through `lan2`, and SSH reached it through the
switch. That happened on three consecutive boots (see below). The CPU port has
no `fixed-link`: DSA skips phylink for it, and firmware's forced 1000/full link
on port 5 holds, as `ea-switch.dtsi` expects. Traffic on the second jack
(`lan1`) has not been tested yet; it had no cable.

If DSA doesn't attach, the network init script falls back to DHCP on the master
interface and the primary jack works as before.

The feature is switched off by default, partly to keep an image under the
[copy window](/ea/boot-chain/#the-copy-window). It isn't needed to reach
the network: the always-applied fake-PHY patch gives `eth0` on the primary jack
on its own.

## The CE5300 SSP is Quark's SSP (patch 0009)

The switch hangs off the CE5300's PXA-style SSP (PCI `01:0b.4`, `8086:2e6a`,
MMIO `0xdffe0a00`), driven by mainline `spi-pxa2xx`. That PCI id is shared with
the CE4100, so mainline takes it for a CE4100, which it is not. Patch
`board/ea/common/patches/linux/0009-spi-pxa2xx-ce5x00-ssp.patch` adds a
`CE5X00_SSP` type, as Control4's GPL kernel does. The type is picked when the SoC
host bridge at `00:00.0` is `8086:0c40`, so a real CE4100 is unaffected.

| | mainline CE4100 | CE5300 |
|---|---|---|
| Register layout (SSCR1 TFT/RFT, SSSR TFL) | 2-bit fields | Quark X1000's 5-bit fields |
| FIFO depth | 4 | 32 |
| Bit clock | SCR divisor | Quark's 24-bit `DDS_RATE` at `+0x28`, then SCR |
| Chip select | in SSSR[1:0] | in SSSR[1:0], like the CE4100 |

Four things in 0009 make it work. Each was found on the EA3 itself.

**1. The bit clock is Quark's DDS.** The rate is
`100 MHz × DDS_RATE / 2^24 / (SCR+1)`. Measured with `DDS_RATE = 1000`,
`SCR = 9`: 1024 bits took 1.74 s, against 1.72 s predicted. With
`DDS_RATE = 0` the SSP does not shift at all; it sits with BSY set. So CE5X00
falls through to the Quark cases everywhere, clock included, and the driver
writes `DDS_RATE` for it.

**2. CE5X00 is pxa25x-compatible.** Mainline's `pxa25x_ssp_comp()` lists the
CE4100 and Quark but not a new type, so the driver took CE5X00 for a part with a
receive-timeout register (SSTO) at `+0x28`. It wrote 1000 there at the start of
each transfer and 0 at the end. That is `DDS_RATE`: the first transfer worked
and the second found the clock stopped. **This was the long-running "kexec
wedge"** (`SSSR = 0x00001F14`, BSY stuck, `probe ... failed with error -22`).
It never had anything to do with kexec.

**3. The chip select lives in SSSR.** `write_SSSR_CS()` must keep CE5X00's chip
select bits too. Otherwise every status clear moves it back to CS0, and the
switch is on CS1.

**4. CEFDK leaves the SSP dirty.** On the way to the kernel CEFDK leaves
`DDS_RATE = 0` and about 25 stale words in the TX FIFO, which toggling SSE does
not clear. At probe, 0009 sets a clock briefly and drains the FIFO before the
first real transfer.

### The chip select drops between transfers

With the clock fixed, b53 still read `0xff`. The SSP's chip select is its frame
signal, and it releases whenever the TX FIFO runs dry, which is between any two
`spi_transfer`s. b53's SPI reads are a command transfer followed by a read
transfer, so the switch sees two separate frames. Driving the SSP by hand proved
it: the command and the read back to back returned RACK and chip id
`25 31 05 00`, and the same bytes split in two returned `0xff`. Control4's own
`spi-bcm53125` driver uses a single full-duplex transfer for exactly this
reason.

The fix belongs in the controller, not in b53. A GPIO chip select (`cs-gpios`)
would be the usual answer, but the CE5300's pin mux
(`ce5300_set_multi_function`) has no GPIO function on the SSP pins. So for
CE5X00 only, 0009 merges a message's transfers into one bounce-buffer transfer:
`optimize_message` builds the merged transfer, `prepare_message` gathers the TX
bytes, and `unprepare_message` scatters the RX bytes back. A message whose
transfers differ in speed or word size, or that asks for a CS change or delay in
the middle, is left alone.

### How 0009 got there

Earlier versions of 0009 got the clock wrong in every possible direction. An
early version used `CE4100_SSP`, and every byte read `0xff`. Aliasing only some
of the Quark cases timed out. The version on main treated CE5X00 as a 10 MHz
SCR-only part with no DDS (PR #69), and PR #92 then flushed the FIFO without
bailing. Both rested on the belief that the CE5300 has no DDS, and that belief
came from the SSTO clobber in point 2: the register was being zeroed by the
driver, not missing. The pre-kexec register dump that seemed to show a live SSP
being wedged by the fat kernel's probe was the same clobber.

## Hardware test, 2026-10-28

The EA3 at 10.0.0.113 ran a one-shot kexec boot of the CI kernel from this patch
(`aef67ed`) with `ohc.switch=1`, three times in a row. The eMMC kernel slot was
never written.

```
pxa2xx_spi_pci 0000:01:0b.4: SSP type CE5X00
b53-switch spi92.1: found switch: BCM53125, rev 4
b53-switch spi92.1 lan1 (uninitialized): PHY [dsa-0.0:01] driver [Generic PHY] (irq=POLL)
b53-switch spi92.1 lan2 (uninitialized): PHY [dsa-0.0:02] driver [Generic PHY] (irq=POLL)
DSA: tree 0 setup
b53-switch spi92.1 lan2: Link is Up - 1Gbps/Full - flow control rx/tx
```

b53 bound on all three boots. On the first boot the box got no address, and
that turned out to be the init scripts:

- switchd's `br-lan` runs STP, so `lan2` only forwards after 2 × forward_delay,
  at 36 s. S40net's DHCP had given up 4 s earlier.
- It then unbound b53, and that doesn't undo b53's switch setup, so plain `eth0`
  got no lease either.

Both are fixed in S40net (PR #110). With that fix, boots 2 and 3 leased
10.0.0.113 on `br-lan` through `lan2`.

:::note[Status]
b53 works on the durable kexec boot. The switch stays opt-in with `ohc.switch=1`,
which S00kexec now carries across the kexec (PR #110). Still open:

- traffic on the second jack (`lan1`);
- switchd's `vlan_filtering 0` call, which this kernel answers with
  `Operation not supported`;
- how stock firmware gets a nonzero `DDS_RATE`.
:::
