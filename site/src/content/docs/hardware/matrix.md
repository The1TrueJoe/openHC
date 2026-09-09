---
title: The controller matrix
description: Every Control4 controller side by side, with the evidence behind each cell.
sidebar:
  order: 1
---

Evidence-based comparison of the six target controllers. `?` marks a cell that
has **not** been verified, do not treat those as fact.

Sources: live pulls off an [EA-1](/ea/ea1/), an [EA-3](/ea/ea3/),
a [CA-1](/ca1/) and an [HC-800](/hc800/); the HC800's `sda.img`;
the `.flash.config` IO firmware manifest; the
[per-board profile table decoded out of the Tiva image](/shared/io-mcu/#the-per-board-profile-table);
and Control4 spec doc DOC-00031-C.

<div class="ohc-wide">

| | **EA-1** | **EA-3** | **EA-5** | **HC-800** | **HC-250** | **CA-1** |
|---|---|---|---|---|---|---|
| Arch | i686 | i686 | i686 ? | **x86_64-capable** | ARMv7 | ARMv7 |
| SoC | Atom CE5310 | Atom CE5310 | Atom CE ? | **Atom D525 + NM10, 2c/4t** | ARMv7 ~1 GHz | **i.MX6SL, 1x A9** |
| Board codename | ninjago | ninjago | garmadon | **none** | ? | emmet |
| Board type strap | 1 | 2 | 3 ? | n/a (type 0) | n/a | n/a (type 0) |
| RAM | ~1.5 GB | ~1.5 GB | ? | 2 GB | 512 MB | 1 GB |
| Storage | 7.6 GB eMMC | 7.6 GB eMMC | eMMC ? | **8 GB SATA SSD + eSATA** | ~2 GB ? | **3.7 GB eMMC + 16 MB SPI-NOR** |
| Boot | CEFDK (no GRUB) | CEFDK 36-34 | CEFDK ? | **AMI BIOS → GRUB 0.97** | U-Boot ? | **U-Boot 2014.04 (SPI-NOR)** |
| Secure boot | **fuse clear** | **fuse BLOWN** | ? | **none — nothing verified** | ? | **HAB open — not enforced** |
| **IO MCU** | TM4C1231D5 | TM4C1231D5 | TM4C1231D5 | **LM3S1162** | LM3S1162 | **none** |
| IO MCU UART | ttyS1 @ 460800 | ttyS1 @ 460800 | ttyS1 ? | ttyS3 @ 115200 | ? | n/a |
| MCU profile block | 2 | 3 | 0/1/4/5 ? | n/a | n/a | n/a |
| IR out (total) | 5 | 7 | 9 ? | 6 ? | 4 ? | 0 |
| IR jacks | 4 | 6 | 8 ? | 6 | 4 ? | 0 |
| Relays | 0 | 1 | 4 ? | 4 | 4 ? | 0 |
| Contacts | 0 | 1 | 4 ? | 4 | 4 ? | 0 |
| RS-232 | 2 combo | 3 combo | 2 ? | **2 host UARTs** | ? | **1 combo 232/485** |
| Ethernet | 1x e1000 | **e1000 + BCM53125 (2 ports)** | ? | 1x RTL8168 (r8169) | 1x | 1x FEC (RMII) |
| Zigbee SoC | EM357 ? | EM357 ? | EM357 ? | EM357 ? | EM357 ? | EM35x ? |
| Zigbee attach | USB-CP2104 | USB-CP2104 | USB ? | on-SoC UART (ttyS4) | UART ? | on-SoC UART (ttymxc4) |
| Z-Wave | ? | ZM5304 | ? | **none on-board** | ? | ZM5304 (ttymxc3) |
| Wi-Fi | ath9k | **none** | ath9k ? | USB RTL8192SU (r8712u) | ? | RTL8723BS (SDIO) |
| Audio DSP | ADAU1451 | ADAU1451 | **FPGA** | none | ? | none |
| Analog codec | ? | AK4621EF | ? | ALC888-VD (HDA) | ? | none |
| Audio out | ? | line + coax + HDMI | ? | 2x line + coax S/PDIF | ? | none |
| Video out | HDMI (Intel GDL) | HDMI (Intel GDL) | HDMI ? | **ADV7513 + THS8200 fitted** | ? | none (headless) |
| GPU | PowerVR SGX (closed) | PowerVR SGX + GC300 | PowerVR SGX | Intel GMA 3150 / i915 | ? | GC320 2D, unused |
| Android LXC | yes | yes | yes | no | no | no |

</div>

## Footnotes that change how you read the table

### IR counts come from firmware, not from the panel

"IR out (total)" is the number of MCU output channels a board populates; "IR
jacks" is the rear-panel count. The difference is one internal front blaster.
Both numbers come from the decoded Tiva profile table.

The EA family's combo RS-232 ports hang off the Tiva and are driven with
`UART_SET_CONTROL` / `UART_SEND` / `UART_RECEIVE` — there's **no host
`/dev/ttyS*`** for them, unlike the HC-800's two 8250 ports.

### The EA5's FPGA is why every EA carries FPGA drivers

The shared "ninjago" kernel ships `snd_ninjago_fpga*` on every EA board. The
image it wants is `garmadon-fpga-45t-spi-rev8.bin`, and **garmadon is the EA5's
codename**. An EA3 has no FPGA at all, the drivers load and bind nothing.

Do not read `lsmod` as evidence here. Check `/sys/bus/*/drivers/ninjago-fpga*/`
for bound devices instead; on an EA3 every one of those directories contains
only `bind`/`unbind`/`uevent` and no device symlinks.

### Relays and contacts fall out of the same profile table

They come from two four-pin groups (`PF0/PF1/PF3/PF4` and `PA2/PA3/PA4/PA5`).
The EA1 populates none of them, the EA3 populates one of each, and the
nine-output blocks populate all eight, which is the HC800/HC250 complement.
**Which group is relays and which is contacts is not established.**

### The HC-800's IR figure is actively disputed

The HC-800 and HC-250 run a different MCU image (`IoProcMultiConfig1162`) and no
profile table has been pulled out of it. The 6 IR / 4 relay / 4 contact counts
here are the **rear-panel counts reported by the owner of the recon unit**,
which is why they carry no emphasis. Relays and contacts agree with the decoded
Tiva table's 4+4 for the nine-output blocks; **IR doesn't** — the
[IO-MCU page](/shared/io-mcu/) reads the LM3S image as four IR channels
from its use of TIMER0–3.

Those are reconcilable, because every Stellaris GPTM has two capture/compare
outputs, so four timers can drive up to eight channels. Until the pin table is
decoded the way the EA one was, **neither number is firmware evidence.**

### The HC-800's two rear RS-232 jacks are host UARTs

Unlike the EA family's MCU-routed combo ports, `ioserver` on the HC-800 opens
`/dev/ttyS1` and `/dev/ttyS2` directly, alongside the MCU on `ttyS3`. This
conflicts with the LM3S image appearing to drive "the two user ports" on its
own UART1/UART2, and a running system can't tell a direct wire from a bridge
through the MCU. Recorded as unresolved.

### The HC-800 is 64-bit and the vendor did not use it

`/proc/cpuinfo` reports `lm` and `nx`. The vendor ships a **32-bit non-PAE**
kernel anyway — `NX protection cannot be enabled: non-PAE kernel!` — which
strands 1149 MB of the 2 GB in HIGHMEM and turns NX off. openHC builds this
board x86_64. It is the only board in the tree where we deliberately do not
match the vendor's architecture.

### There is no `sherman`

Earlier revisions of this table listed `sherman` as the HC-800's codename.
Nothing on a live unit uses that name: `/proc/c4board/name`, the filesystem
labels, the kernel version string and `.flash.config` all just say `hc800`. Its
`type` is 0, and the three GPIO straps the EA family would use for a board type
instead carry the **board revision**.

### The HC-800 is not headless at the silicon level

A TI THS8200 video DAC and an ADI HDMI transmitter are fitted, answer on SMBus,
and the vendor stack drives them to 720p at every boot. The panel on the recon
unit has no video connector its owner uses, and openHC builds no video for this
board, but "headless" was wrong about the hardware.

### Secure boot, board by board

The EA1 (board **v1**) boots unsigned kernels on the normal path; the EA3 (board
**v2**) does not. The fuse was likely blown starting at some board revision,
which means an **EA3 v1 may behave like the EA1**. Untested.

The CA-1's HAB is Open on three independent grounds, derived on the
[CA-1 page](/ca1/#secure-boot-hab-is-open-and-nothing-else-is-verified-either).
The HC-800 verifies nothing anywhere in its chain — AMI BIOS to GRUB 0.97 to a
bare bzImage named in a plain-text file on an ext3 partition. It is the least
locked-down boot chain in the table, and the reason openHC installs on it by
copying two files and adding a menu entry.
