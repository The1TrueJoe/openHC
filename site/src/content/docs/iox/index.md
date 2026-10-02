---
title: IO Extender V1
description: The TI DaVinci DM355 "hammer" board — the one with real on-board IO, and an SoC mainline deleted.
sidebar:
  order: 1
  label: Overview & recon
---

The Control4 IO Extender V1 — "Control4 DM355 I/O Bar", board codename
**`hammer`**, is the board this project started for. It is also the only one
whose SoC no longer exists in mainline Linux.

## Identity

```
SoC        TI DaVinci DM355 (ARM926EJ-S, ARMv5TE)
RAM        128 MB
Storage    512 MB Micron NAND (stock partitions use the first 256 MB)
Kernel     Linux 2.6.28.10-rc8.47   (OS 2.9.1.539509-res)
Userland   BusyBox 1.2.2 + glibc 2.7 (EABI)
Bootloader U-Boot 1.2.0-IOX
```

Dropbear 2016.73 on this unit needs legacy algorithms from a modern OpenSSH:

```
-o HostKeyAlgorithms=+ssh-rsa -o KexAlgorithms=+diffie-hellman-group1-sha1
```

ICMP ping is flaky or fails on these units; SSH works regardless.

## The IO, which is the point of the board

Unlike the EA and HC families, **there's no companion microcontroller.** Relays,
contacts, buttons and LEDs are plain SoC GPIO, so driving them needs no driver at
all — just libgpiod. Only the serial ports and IR outputs sit behind an FPGA.

| Function | Count | How |
|---|---|---|
| Relays | 8 | SoC GPIO 88–95, active high — **confirmed clicking** |
| Contacts | 8 | SoC GPIO 70, 71, 82–87 |
| IR out | 8 | FPGA at `0x04000220` / `0x04000230` — **blocked, FPGA unprogrammed** |
| RS-232 | 4 | 16550A UARTs **inside the FPGA** — **blocked, same reason** |

Video-sense hardware is not shipped on this unit.

The stock system exposes the IO through Control4's own drivers —
`/sys/class/c4relay/rlyN/state`, `/sys/class/c4contact/cntN/state` — which is a
perfectly reasonable path if you only want to replace userspace. openHC replaces
the kernel.

Full detail: [the IO map](/iox/io-map/).

## Memory map

```
SoC async EMIF control      0x01e10000
NAND data (CE0)             0x02000000, 32 MB window, 14 MTD partitions
FPGA on EMIF CE1            0x04000200, 0x100 window
  IR out 0/1                  +0x20 / +0x30
  UART0..3                    +0x40 / +0x50 / +0x60 / +0x70  (0x10 each, 16550A)
  AC97 (unused)               +0x80
Ethernet dm9000             0x04014000 (addr) / 0x04014002 (data)
GPIO controller             0x01c67000
```

The **dm9000 has no EEPROM**, so the MAC must be forced, via a
`local-mac-address` property or the kernel command line. Its IRQ is GPIO 1 and its
reset is GPIO 101, active low. DM355 GPIO 0–9 are unbanked and route directly to
the interrupt controller.

## Flash: dual-bank plus recovery

The unit boots bank 1. All the MTD tools are already on the box —
`flash_erase`, `nandwrite`, `flashcp`, `fw_setenv` — and dual banks plus a
recovery image mean **brick risk is low even for a bad flash**.

Better still, nothing needs to be flashed during bring-up. See below.

## Netboot: `run tst`

The stock U-Boot environment already contains `tst`, which does DHCP, TFTPs
`hammer/uImage` from the server, and `bootm`s it **from RAM**. No flash writes.
That is the entire development loop for this board.

:::caution[`run tst` never falls through]
If DHCP succeeds but TFTP does not answer, U-Boot retries forever. It ignores
ICMP port-unreachable, and on a TFTP *error* it prints "Starting again" and
retries too, because `netretry` is not set to `no`. So a refusing server does
not make it boot stock. Recovery to stock needs a U-Boot prompt, or serving the
stock kernel itself (pull `kernel1` off NAND, correct it with 1-bit ECC, and
`tst` boots it with the stock rootfs since it runs `setbootargs` first).
:::

### Booting it with no serial console at all

The armed `bootcmd` retries TFTP forever and its `tst` has a hardcoded server
address of `192.168.0.10`. That's exploitable:

```sh
ifconfig <lan-if> alias 192.168.0.10 255.255.255.0
ohc-flash netboot --board ioxv1 --mac <box-mac> --image openhc-ioxv1-kernel.img
```

A MAC-filtered DHCP responder races the real LAN server and offers the board
`192.168.0.50/24`. It ACKs every request from that MAC, even one that picked the
router's offer, because the client keeps whichever ACK lands first. Once the
router has a lease for the MAC it answers faster than anything on Wi-Fi, so
delete that lease to keep winning. That puts it on the same subnet as the
alias, so the board's hardcoded TFTP goes **direct**, with no dead gateway hop, and
lands on us. The board boots our kernel over the main LAN with **no
point-to-point adapter and no U-Boot prompt**.

This exists because USB ports are scarce in practice: with several hardware
projects running at once, a USB-Ethernet adapter and a USB-UART cannot both be
assumed available. The end goal is a **pure network** bring-up — netconsole over
the on-board Ethernet plus SSH, so debugging visibility never depends on a UART.

To boot **stock** without a U-Boot prompt, serve the stock kernel instead: dump
`kernel1` raw with its OOB, apply the 1-bit ECC to get a CRC-clean uImage, and
`tst` boots it with the stock rootfs (it runs `setbootargs` first). A refusing
TFTP server does not work: on an error U-Boot prints "Starting again" and loops.

## Current state

**The openHC kernel boots on real silicon.** Linux 7.1.8 reaches userspace and
prints `openhc-ioxv1 login:`.

**Everything on the board works**, installed on the NAND:

| Function | State |
|---|---|
| Ethernet, SSH, iod, web UI | working |
| 8 relays, 8 contacts | working through iod and MQTT (contacts active-low) |
| 4× RS-232 (`ttyS1`–`ttyS4`) | verified both ways against a PC, raw and through iod |
| 8 IR outputs | all eight verified against a GC-IRL learner (NEC at 38 kHz), no crosstalk |
| LEDs | data, link and the tri-colour status LED; power is hardwired |
| Flash install | `tools/ohc-ioxv1 install`, from stock or openHC, over SSH; falls back to stock after 3 failed boots |

iod holds every RS-232 port open from boot with DTR and RTS up, as the stock
`dtserver` does, because accessories like the GC-IRL are powered from those
lines. `serial/N/dtr` and `serial/N/rts` drop either one.

Two fixes were required to get there, and both are the kind that present as
something else entirely:

**GPIO must be banked in the device tree.** With `ti,davinci-gpio-unbanked = <0>`
and seven bank interrupts wired to AINTC, everything works. In unbanked mode the
driver registers **no interrupt domain at all**, so `dm9000`, which names the
GPIO controller as its interrupt parent, can't resolve its interrupt and fails
with `-517`, "IRQ index 0 not found". The error names an IRQ; the cause is a
domain that was never created.

**libgpiod 1.6.4 needs `CONFIG_GPIO_CDEV_V1`**, which defaults **off** in 7.1.8.
Without it `gpioset`, `gpioget` and `gpioinfo` all return `EINVAL`, tools that
look broken against a kernel that's fine. Legacy sysfs GPIO additionally needs
`GPIO_SYSFS_LEGACY`.

The serial console on this unit is **read-only** — TX isn't wired, so the login
getty respawns on floating-RX noise. Cosmetic, but it also means U-Boot can't be
interrupted from serial here.

## The FPGA

The four RS-232 ports and eight IR outputs live in a Xilinx Spartan-3E that
comes up blank. `ohc-iox-fpga` loads it at boot (`S12fpga`), bit-banging
`fpga_fw.bin` over slave-serial GPIO. The bitstream is proprietary, so it is not
in the image. `S12fpga` copies it, read-only, from the unit's own recovery
rootfs on NAND.

That only works with the NAND ECC set right (1-bit, not 4-bit). Getting there
took a while; [the FPGA startup problem](/iox/fpga-startup-clock/) has the
whole story.

## NAND layout

The stock flash holds three complete systems, each a kernel plus a 64 MB JFFS2
rootfs: update banks 0 and 1 (A/B; `bootsystem` in the U-Boot env picks one)
and a factory **recovery** pair. Beside them sit the writable `jffs2.img`,
`Internal` and `Persistent Logs` partitions. The chip is 512 MB but the stock
table covers only the first 256 MB. The top half is where openHC installs: a
32 MiB slot at `0x10000000` holds the image, and the last two blocks hold the
bad-block tables. openHC's device tree names that half `openhc` (writable) and
keeps every stock partition read-only.

The ECC is DaVinci **1-bit**, three bytes per 512-byte sector at OOB 40–51,
stored inverted. U-Boot, the stock kernel and openHC all agree on it now, so an
image openHC writes is one U-Boot reads.

## Vendor drivers, for reference

The stock kernel carries `c4relay`, `c4contact`, `c4irout`, `c4serial`, `c4fpga`,
`c4button` and `fpdriver`.

Control4's GPL drop includes the board files, headers and the U-Boot FPGA loader,
but **omits the `c4fpga.c` / `c4gpio.c` / `c4irout.c` driver bodies**, partial
compliance. It's not a blocker, because the digital IO needs no driver and the
FPGA loader protocol and register windows are both documented in the board file
that *was* published, but it is worth recording plainly. See
[the GPL drop](/shared/gpl-source/).

## Pages

- [IO map](/iox/io-map/) — the authoritative pin and register map,
  including the pinmux writes without which none of the GPIO does anything.
- [The 7.1 kernel port](/iox/kernel-port/), what had to be resurrected,
  and the patch-by-patch log.
