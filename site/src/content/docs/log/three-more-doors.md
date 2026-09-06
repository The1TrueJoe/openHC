---
title: Three more doors
topic: Recon
summary: The CA-1, the HC-800 and the IO Extender. Each one opened by something already sitting in its stock boot path, waiting to be used.
description: How the non-EA boards were opened, and why none of them needed a bypass.
sidebar:
  order: 7
  label: Three more doors
---

The EA work took months and ended in a signature bypass, an autoscript and a 7 MB
budget. The reasonable expectation for three more boards was three more months.

It wasn't, and the reason is worth saying up front. On all three, the stock
firmware already contains a path that loads unsigned code, and on two of them the
vendor left that path completely unused. The work was recon, not defeat.

## The CA-1: a file the bootloader looks for and never finds

The CA-1 runs stock U-Boot 2014.04 out of SPI-NOR. Here's its `bootcmd`, read off
the environment:

```
if mmc rescan; then
  if run loadbootscript; then run bootscript;      # ① boot.scr on p1
  else if run loadimage; then run mmcboot;         # ② zImage on p1
       else run loadtftp; fi;                      # ③ TFTP
  fi;
else run netboot; fi
```

`loadbootscript` is `fatload mmc 1:1 ${loadaddr} boot.scr` and `bootscript` is
`source`. There's no `boot.scr` on the unit. The bootloader checks for one first,
every single boot, and finds nothing.

Dropping a file onto the eMMC's vfat partition is therefore a complete, reversible
takeover that needs no serial console, no soldering and no bootloader reflash.
Delete the file to go back to stock.

That mattered more than usual here, because the CA-1's U-Boot console is
password-locked with the same SHA-256 gate as CEFDK, with the identical baked-in
digest. There's no way to a U-Boot prompt over serial at all. Luckily
`fw_printenv` and `fw_setenv` are both in the vendor rootfs with a valid config,
so the environment is readable and writable from Linux, which is the only way to
change it.

### Secure boot, settled three ways

The obvious worry was i.MX High Assurance Boot. It's Open, on three independent
grounds.

**The fuse.** `HW_OCOTP_CFG5` reads `0x00000000`; `SEC_CONFIG[1]` is the bit, and
`0` is Open.

**U-Boot is built for signing but was never signed.** The IVT at flash offset
`0x400` parses cleanly and its CSF pointer is populated, pointing at a slot that
is 8 KB of zeros. No `0xD4` tag, no certificate, no signature. The binary contains
the HAB machinery (`hab_status`, `hab_auth_img`, `"Secure boot enabled"`), so it
was compiled with `CONFIG_SECURE_BOOT` and reserves the space; the build just
never ran the signing tool. A Closed part would parse that zero-filled slot, fail,
and refuse to boot. The unit boots, so the part is Open, which agrees with the
fuse independently.

**Even a closed ROM wouldn't cover the kernel.** HAB authenticates only what the
ROM loads. `bootcmd` never calls `hab_auth_img`; the kernel is a plain `bootz`
from a FAT partition. No dm-verity, no signed rootfs, no measured boot anywhere.

There's a caveat on reading the fuse that's its own small lesson. The vendor
`fsl_otp` driver returns `0xbadabada` for one fuse and then reads empty for every
fuse after it, staying wedged until reboot. `CFG5` was read before the driver
wedged. Cross-checking against the OCOTP shadow registers wasn't possible either,
because `read()` on device memory returns `EFAULT` on ARM. It needs `mmap`, and
the rootfs has no interpreter and no `devmem` applet.

### And the device tree is a lie in three places

Control4 started from `imx6sl-evk.dts` and never changed the `compatible`, which
still reads `"fsl,imx6sl-evk"`. Several devices in the vendor DT simply aren't on
this board: an Elan touchscreen, an accelerometer, an E-ink PMIC.

Two more that actually bite. `usdhc1`/`usdhc2` carry `cd-gpios`/`wp-gpios`, and
there's no card slot and no write-protect switch on either; both are soldered
parts. And the `bus-width` properties are backwards relative to the pinmux. The DT
says usdhc1 is 8-bit and usdhc2 is 4-bit; the pin groups say usdhc1 has 6 pins
(4-bit) and usdhc2 has 10 (8-bit). The pins are ground truth: usdhc1 is the 4-bit
SDIO radio, usdhc2 is the 8-bit eMMC.

Trust the pin groups, not the properties.

### Recovery, learned the hard way

A broken `boot.scr` leaves the box in a watchdog loop with no network and no
U-Boot console. The way out is all pre-`bootcmd` and needs no password: hold the
recessed factory-restore button at power-on for about ten seconds, and U-Boot's
`check_factoryrestore()` reads that pin *before* `bootcmd` runs and boots the
recovery kernel from SPI-NOR.

The recovery kernel's initramfs then offers a two-second `c4`+ENTER break-in to a
root shell, and unlike U-Boot that one isn't password-gated. From there, mount p1
and delete the offending file.

Worth knowing: a *full* factory restore reimages the rootfs and rewrites the stock
kernel, but doesn't delete extra files like our `boot.scr`. So the restore alone
doesn't break the hang loop. You have to use the shell.

## The HC-800: a text file

The HC-800 turned out not to be an embedded board at all. It's a PC, and its
entire boot chain is AMI BIOS → GRUB 0.97 → a bare bzImage named in
`/boot/grub/menu.lst` on an ext3 partition that mounts over SSH.

Nothing is verified. No secure boot, no signed kernel, no container format, no
measured boot. It's the least locked-down boot chain in the line.

The vendor's own layout makes the install cheap, for a reason that's interesting
in itself: GRUB 0.97 can't read ext4 extents, so the kernel can't live on the ext4
root. Control4 solved that by giving the kernel its own small ext3 partition,
`sda3`, which has 165 MB free and which GRUB can read every byte of.

So installing openHC is three file operations. Copy a bzImage and a `cpio.gz` onto
`sda3:/boot`, append a third entry to `menu.lst`, change `default` from `1` to
`2`. Both vendor entries stay byte-identical and the factory-restore partition is
never touched. Recovery is changing one digit back.

Two things had to be confirmed rather than assumed. `initrd` works, verified by
string-dumping the installed `stage2`, which carries the `initrd FILE` builtin and
the `[Linux-initrd @ 0x%x, 0x%x bytes]` loader message. And `timeout 0` plus
`hiddenmenu` means no interactive menu appears, so selecting a different entry
means editing the file, not catching a prompt.

The other headline is that this board needs no kernel patches at all. Every driver
is mainline and has been for a decade. There's one trap: mainline defaults
`CONFIG_SERIAL_8250_RUNTIME_UARTS` to 4, and this board has five UARTs, so
`ttyS4`, the Zigbee radio, silently doesn't appear unless *both* `NR_UARTS` and
`RUNTIME_UARTS` are set to 5.

## The IO Extender: a command already in the environment

The oldest board in the set, a 2009-era DM355 running a 2.6.28 kernel, has the
friendliest development loop of any of them. Its stock U-Boot environment already
contains `tst`, which does DHCP, TFTPs a kernel and boots it from RAM.

No flash writes, dual flash banks plus a recovery image, and every MTD tool
already on the box. Low brick risk before I started.

It has one sharp edge, found the hard way. `run tst` does not fall through to the
stock boot command on a *silent* TFTP failure. If DHCP succeeds but TFTP doesn't
answer, U-Boot loops forever; it ignores ICMP port-unreachable. Recovery needs
either no DHCP at all, so `tst` aborts, or a definitive TFTP *error* reply. The
armed command is `run tst; run oldbootcmd` with the original saved, and that
fall-through only fires on failures U-Boot recognises.

### Booting it with no serial console at all

This board also produced the most useful trick in the project, and it came out of
a constraint that has nothing to do with hardware. With several projects running
at once there are rarely enough free USB ports for both a USB-Ethernet adapter and
a USB-UART. Assume only one is available.

The armed `bootcmd` retries TFTP forever, and its `tst` has a hardcoded server
address of `192.168.0.10`. So:

```
ifconfig <lan-if> alias 192.168.0.10 255.255.255.0
netboot.py --board ioxv1 --iface <lan-if> serve
```

A MAC-filtered DHCP responder races the real LAN server, the NAK logic helps it
win, and offers the board `192.168.0.50/24`. That puts it on the same subnet as
the alias, so its hardcoded TFTP goes direct, with no dead gateway hop, and lands
on us. The board boots our kernel over the main LAN with no point-to-point adapter
and no U-Boot prompt.

The end goal it serves is a pure network bring-up: netconsole over the on-board
Ethernet plus SSH, so debugging visibility never depends on a UART. Serial is a
temporary loan from another project, not the target workflow.

## The pattern

Three boards, three completely different bootloaders, and the same shape of answer
each time. Nobody defeated anything. The vendor's own boot path had an unused
branch, a file that's never present, a menu entry that can be appended to, an
environment command left armed from the factory, and taking it was cheaper and
far safer than fighting the path they did use.

The EA3 is the sole exception, and even there the
[answer was a road already next to the wall](/log/the-fuse-at-the-wrong-address/),
not a hole in it.
