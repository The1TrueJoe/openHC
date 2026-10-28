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

**DSA's bring-up of the isolated second jack is still unproven on hardware.** If
DSA doesn't attach, the network init script falls back to DHCP on the master
interface and the primary jack works as before, which is the behaviour the
`switch` feature being currently disabled relies on.

The feature is switched off at present to keep an image under the
[copy window](/ea/boot-chain/#the-copy-window). It isn't needed to reach
the network: the always-applied fake-PHY patch gives `eth0` on the primary jack
on its own.

## The CE5300 SSP: Quark's registers, not Quark's clock (patch 0009)

The switch hangs off the CE5300's PXA-style SSP (PCI `01:0b.4`, `8086:2e6a`),
driven by mainline `spi-pxa2xx`. That PCI id is shared with the CE4100, so
mainline takes it for a CE4100, which it is not. Patch
`board/ea/common/patches/linux/0009-spi-pxa2xx-ce5x00-ssp.patch` adds a
`CE5X00_SSP` type, as Control4's GPL kernel does. The type is picked when the SoC
host bridge at `00:00.0` is `8086:0c40`, so a real CE4100 is unaffected.

| | mainline CE4100 | CE5300 |
|---|---|---|
| SSCR1 TFT / RFT | 2-bit fields | 5-bit fields (bits 10:6 / 15:11) |
| SSSR TFL | bits 9:8 | bits 12:8 |
| FIFO depth | 4 | 32 |
| SSP input clock | 3.6864 MHz | **10 MHz** |
| Bit clock | SCR divisor | SCR divisor, **no DDS** |

The **register layout** is Quark X1000's, so `CE5X00_SSP` falls through to
`case QUARK_X1000_SSP:` everywhere a layout is chosen. It joins `CE4100_SSP`
where the chip select is written into SSSR.

**The clock must not be aliased to Quark.** Quark builds its bit clock from a
24-bit `DDS_RATE` register plus SCR. The CE5300 has no DDS: on its SSP that
offset is not a clock register, and the vendor driver never touches it. Through
Quark's clock path the SSP wedges:

```
b53-switch spi92.1: Flush failed
b53-switch spi92.1: SPI transfer failed: -5
```

So CE5X00 is clocked like a CE4100/PXA25x part: an 8-bit SCR, a bit rate of
`clk / (2 × (SCR+1))`, and a 10 MHz input clock (the vendor's
`spi_clk_get_rate()`). `ce4100_spi_setup()` registers 10 MHz for a CE5300 and
3.6864 MHz otherwise.

How 0009 got there:

- An early version set `CE4100_SSP`. The chip select was right but the FIFO
  layout was wrong, and every byte read back as `0xff`.
- The next version aliased only five of the twelve Quark sites. The rest fell to
  `default:` and transfers timed out.
- Aliasing the clock divider to Quark as well puts CE5X00 on the DDS path,
  which causes the `Flush failed / -5` wedge above. The version that first
  reached main avoided that, but gave CE5X00 neither the /2 SCR divisor nor the
  10 MHz clock. It fell to the generic `clk / (SCR+1)` divider off the CE4100's
  3.6864 MHz, so the bit rate was wrong.
- **PR #69** (`aa54ecb`) made it a 10 MHz SCR part with no DDS. It also skipped
  pxa2xx's flush-first for CE5X00, because that flush polls a stopped SSP until
  it times out. This came from the never-PR'd branch `ea-switch-ce5x00-clock`
  (523266d + 2ef53c3).
- **PR #92** (`b32448c`) changed the skip to "flush but don't bail". It does not
  fix the switch; see the 2026-10-27 test below.

:::note[Rate nit]
`ea-switch.dtsi` asks for `spi-max-frequency = <1843200>`, but this SCR divider
cannot hit 1.8432 MHz from 10 MHz. The divisor rounds up, so SCR = ⌈10 MHz /
(2 × 1.8432 MHz)⌉ − 1 = 2, and the bus actually runs at 10 MHz / 6 =
**1.667 MHz**. `SSCR0 = 0x207` has SCR = 2, which agrees. This is harmless, since
it is under the request, but the number in the tree is not the bus rate.
:::

## When the switch is enabled, b53 still doesn't probe: the kexec-wedged SSP

Turning the `switch` feature on (`ohc.switch=1`) is necessary but, on an
installed box, not sufficient. openHC reaches its real kernel on the EA3 by
**kexec** — the autoscript hands off to the fat kernel past the
[copy window](/ea/boot-chain/#the-copy-window) — and on every kexec boot the b53
probe dies:

```
b53-switch spi92.1: probe with driver b53-switch failed with error -22
```

with the SSP the switch hangs off (the PXA-style controller at PCI `01:0b.4`,
MMIO `0xdffe0a00`) stuck busy:

```
SSSR = 0x00001F14   (BSY, bit 4, set — the bit clock never shifts valid data)
```

A cold **CEFDK-direct** netboot, by contrast, comes up with the SSP alive (BSY
clear). So the silicon and the clock divider are fine — the CE5X00 10 MHz/SCR
clock fix (PR #69) already killed the old `Flush failed / SPI transfer -5` wedge.
Something about the **kexec hand-off** leaves the SSP dead, and it reproduces on
every cold boot (8/8 in testing).

### What it is not — ruled out by reading, stock vs openHC

Every candidate gate reads **identical** between a working stock boot and a dead
openHC boot:

| Suspect | Finding | Verdict |
|---|---|---|
| CP_TOP clock/reset unit (`00:00.2`, `0xbfffef00`) | all ~40 registers byte-identical stock vs openHC | not it |
| P-Unit power islands (`00:1e.0` OSPM) | island word `0x0` — all islands on, same as stock | not it |
| SSP PCI power state (`01:0b.4`) | `D0`, Mem+/BusMaster+ | not it |
| SSP private clock reg (its own BAR0, `0x00`–`0xff`) | only SSCR0/SSCR1/SSSR populated, nothing past `0x2c` | not it |
| `ismdclock` (`01:0c.1`, `0xdffe1000`) | dormant A/V timestamp clock, all-zero on openHC only because no media stack runs it | parallel symptom, not the SSP |
| CRU `+0x94` / `+0x98` | already at stock values on a dead boot; a write is a no-op and b53 still fails | not it |

### What it is — the fat kernel's SPI probe wedges a live SSP

A guarded dump added to `S00kexec`, reading the SSP in the **boot-init kernel
immediately before `kexec -e`** (while CEFDK's setup is still intact), caught the
controller **alive** — and the fat kernel's `spi-pxa2xx` probe then **wedges** it:

```
              pre-kexec (CEFDK-fresh)      post-kexec (fat kernel, dead)
SSCR0  +0x00  0x00000007                   0x00000207
SSCR1  +0x04  0x000003C0                   0x00000000   <- driver clears the FIFO thresholds
SSSR   +0x08  0x0003E005  (BSY clear)      0x00001F14   (BSY set)
+0x24         0x00000003                   0x0000000A   (CE reserved/status reg; a wedge symptom)
CP_TOP        unchanged across the hand-off
```

CP_TOP is unchanged across the hand-off, so **no clock is lost in transit** — the
driver is re-initialising the CE5X00 and killing it. The real deltas are `SSCR1`
going `0x3C0 → 0` (the probe clears the TX/RX FIFO thresholds the vendor sets) and
the `SSCR0`/SSE reconfiguration; the controller ends up busy-stuck. (The true
`SSPSP` register, `+0x2C`, reads `0` on both — untouched, as the vendor expects —
so it is *not* involved; the `+0x24` change is a reserved status reg reflecting
the wedge, and writes to it bounce.) Restoring those register values at runtime
does **not** un-wedge it (BSY stays latched in the engine), so it cannot be
repaired after the fact: the probe has to stop doing the destructive init in the
first place.

### 2026-10-27: "flush but don't bail" (#92) does not fix it

PR #92 assumed a stale RX FIFO inherited from CEFDK was wedging the first
transfer, so it flushed for CE5X00 and ignored the result. It was tested on the
EA3 (10.0.0.113) with its fat kernel booted through the real durable path:
bootstrap kernel → `S00kexec` → kexec. In that build `spi-pxa2xx` is built in
and `b53_spi` is a module that loads at about 38 s.

```
b53-switch spi92.1: probe with driver b53-switch failed with error -22

              pre-kexec (same boot)        after the b53 failure
SSCR0         0x00000007                   0x00000207
SSCR1         0x000003C0                   0x00000000
SSSR          0x0000D905  (live)           0x00001F14  (BSY stuck, TFL=31)
```

The SSP was live before kexec and wedged after it, the same as before #92. **The
stale-FIFO flush theory is ruled out.** Two leads remain:

- **H2:** put CE5X00 on the MMP2 TX-FIFO drain path in `spi-pxa2xx`.
- **H3:** reconfigure a live CE5X00 in place, without the `pxa_ssp_disable()`
  stop/restart.

:::note[Status]
Narrowed to the fat kernel's `spi-pxa2xx` handling of a live CE5X00 SSP after
kexec. It is not a lost clock, and it is not a stale FIFO (#92 is ruled out).
The open work is H2 or H3 in patch `0009`, plus a kexec hardware test. Until then the switch
stays off and `eth0` runs on the primary jack via the fake-PHY path; a switch-off
boot now falls back cleanly (the `S39ea-switch` / `S87ohcswitch` gating fix, PR
#85). The register-level narrowing is also recorded alongside the
[audio bring-up](/ea/audio/), which hit the same closed CE5300 clock-controller
wall.
:::
