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
Storage    256 MB Micron NAND
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

:::caution[`run tst` doesn't fall through on a silent TFTP failure]
If DHCP succeeds but TFTP does not answer, U-Boot loops forever — it ignores ICMP
port-unreachable. Recovery to stock needs **either** no DHCP at all (so `tst`
aborts) **or** a definitive TFTP *error* reply.

The armed command is `run tst; run oldbootcmd` with the original saved, and that
fall-through only fires on the failures U-Boot actually recognises.
:::

### Booting it with no serial console at all

The armed `bootcmd` retries TFTP forever and its `tst` has a hardcoded server
address of `192.168.0.10`. That's exploitable:

```sh
ifconfig <lan-if> alias 192.168.0.10 255.255.255.0
netboot.py --board ioxv1 --iface <lan-if> serve
```

A MAC-filtered DHCP responder races the real LAN server — NAK logic helps it win —
and offers the board `192.168.0.50/24`. That puts it on the same subnet as the
alias, so the board's hardcoded TFTP goes **direct**, with no dead gateway hop, and
lands on us. The board boots our kernel over the main LAN with **no
point-to-point adapter and no U-Boot prompt**.

This exists because USB ports are scarce in practice: with several hardware
projects running at once, a USB-Ethernet adapter and a USB-UART cannot both be
assumed available. The end goal is a **pure network** bring-up — netconsole over
the on-board Ethernet plus SSH, so debugging visibility never depends on a UART.

(Recovery to stock still needs a real U-Boot prompt, which needs TX wired, or an
isolating dumb switch.)

## Current state

**The openHC kernel boots on real silicon.** Linux 7.1.8 reaches userspace and
prints `openhc-ioxv1 login:`.

**SSH and GPIO work.** dm9000 brings up `eth0`, dropbear runs, and `gpiochip0`
exposes 104 lines. All eight relays click and the front data and link LEDs light.

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

## What is still blocked

The **FPGA**, which gates the four RS-232 ports and the eight IR outputs. It is a
Xilinx part loaded by slave-serial bit-bang over seven GPIOs (55, 57, 96, 97, 58,
7, 98), from `fpga_fw.bin` which is on the stock NAND. That protocol is standard
and portable and can live entirely in userspace with libgpiod.

Note that the CE1 window is **not** readable from userspace `devmem` — it needs a
kernel driver or proper AEMIF timing configuration first.

After the FPGA, the only genuinely bespoke driver the board needs is IR-out: two
0x10 register windows, to be recovered from the on-device `c4irout.ko`.

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
