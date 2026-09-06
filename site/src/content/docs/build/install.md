---
title: Installing on a controller
description: The per-board install path — over SSH, over a serial console, or by copying three files.
sidebar:
  order: 2
---

Four boards, four install paths, because each one takes a different route into its
bootloader. **Read [recovery](/shared/recovery/) first.**

The [flasher](/build/flasher/) automates the EA and CA-1 paths and will
identify the board for you. This page is what it does, and what to do by hand.

## Which method applies

| Method | What it does | Boards | Needs |
|---|---|---|---|
| **network** | over SSH into a running system: writes the eMMC container and, on a secure-boot part, the boot autoscript | EA family | an IP address |
| **uboot** | copies `zImage` + DTB + `boot.scr` onto the vfat partition | CA-1 | an IP address |
| **serial** | drives the CEFDK shell over a console | EA family | a USB-serial adapter and the ID button |
| manual | copy two files, edit a text file | HC-800 | an IP address |

The serial method is the **fallback**, not the default. It exists for the case
where nothing is running to SSH into.

## EA family

### Non-destructive first: netboot

Nothing here writes flash, and a power cycle returns the unit to stock.

```sh
make image BOARD=ea3-v2
make netboot BOARD=ea3-v2
```

`make netboot` serves BOOTP and TFTP, waits for the CEFDK shell to appear, then
drives the whole `bootlinux` sequence over the serial console itself and streams
the boot log to `output/boot-console.log`.

The **only** thing it can't do is hold the **ID button** for you, so it prints a
reminder and waits (default 300 s). It needs `sudo` because BOOTP and TFTP are
privileged ports, and a USB-serial adapter on the console header.

To get a bootloader shell instead of booting a kernel: `make probe`, then the same
ID-button power-cycle.

Full protocol detail: [bootloader access](/ea/bootloader-access/).

### Persistent, over the network

The two-stage install writes the kernel and initramfs, reboots the unit into RAM,
then writes the rootfs to p1 and reboots into that. The destructive step happens
while running **from RAM**, not from the filesystem being replaced.

```sh
ohc-flash identify <host>
ohc-flash plan ea3-v2                     # what it would do, without doing it
ohc-flash install <host> --images output/images --yes
```

What it establishes, all reversible via the restore button, with **p2 never
written**:

- the signed stock kernel stays in the container as a `script off` recovery;
- our kernel goes in the ~25 MB raw gap between the container and p1;
- our ext4 rootfs goes on `mmcblk0p1`;
- on a secure-boot part, the autoscript goes in SPI-NOR as an MFH item.

On an **ea1-v1** the fuse is clear and the plain container path works with no
autoscript. On an **ea3-v2** the fuse is blown and the autoscript is required —
see [secure boot](/ea/secure-boot/).

:::caution[Install to the raw gap with CEFDK's `emmc wr`, not Linux `dd`]
Linux `/dev/mmcblk0` and CEFDK's `emmc` command don't agree on the raw gap's byte
offset. The container at `0x400` coincidentally matches; `0x800000` doesn't.
Partition p1 is a real partition, so `dd` is fine for it — only the raw gap needs
the bootloader's own addressing.
:::

### Going back

Press the recessed factory-restore button. It reimages kernel, rootfs **and**
CEFDK from p2. Or, if the box still boots, `script off` at the CEFDK shell
restores the stock verified boot path without touching anything else.

## CA-1

The cleanest install of the four, because the stock `bootcmd` already looks for a
file that doesn't exist.

```sh
make image BOARD=ca1
```

Then copy `zImage`, the DTB and `boot.scr` onto the eMMC's **vfat p1**. The
bootloader tries `fatload mmc 1:1 ${loadaddr} boot.scr; source` before the stock
kernel on every boot, so the file takes over immediately.

**Delete `boot.scr` to go back to stock.** Nothing else changes.

There's also a RAM-only path — `make netboot BOARD=ca1`, holding the ID button at
power-on for manufacturing mode — which writes no flash at all.

:::danger[The U-Boot console is password-locked]
There is no way to a U-Boot prompt over serial on this board, the autoboot
break-in is SHA-256 gated. If a bad `boot.scr` hangs the box, recovery is the
**factory-restore button plus the recovery kernel's `c4` shell**, and a full
factory restore alone will *not* fix it because it doesn't delete extra files.
See [CA-1 recovery](/ca1/#recovery-when-a-bad-bootscr-hangs-the-box).
:::

## HC-800

Three file operations, fully reversible, and the vendor rootfs is never written.

```sh
make image BOARD=hc800
```

That prints the exact commands. In outline:

```sh
# 1. kernel + initrd onto the ext3 kernel-only partition
ssh root@<ip> 'mkdir -p /mnt/k && mount /dev/sda3 /mnt/k'
cat openhc-bzImage   | ssh root@<ip> 'cat > /mnt/k/boot/openhc-bzImage'
cat openhc-initrd.gz | ssh root@<ip> 'cat > /mnt/k/boot/openhc-initrd.gz'
ssh root@<ip> 'umount /mnt/k'

# 2. append our stanza to sda1's menu.lst as a THIRD entry, back up the original,
#    and point default at it (entries are 0-based, so ours is 2)
ssh root@<ip> 'mkdir -p /mnt/g && mount /dev/sda1 /mnt/g &&
                cp /mnt/g/boot/grub/menu.lst /mnt/g/boot/grub/menu.lst.stock'
cat menu.lst.openhc | ssh root@<ip> 'cat >> /mnt/g/boot/grub/menu.lst'
ssh root@<ip> 'sed -i "s/^default.*/default\t\t2/" /mnt/g/boot/grub/menu.lst &&
                umount /mnt/g && reboot'
```

Both vendor entries stay byte-identical and the factory-restore partition on
`sda2` is never touched. **Recovery is setting `default` back to `1`**, or
restoring `menu.lst.stock`.

:::caution[Plain `scp` doesn't work against these units]
Modern OpenSSH `scp` speaks SFTP and the vendor's dropbear has no `sftp-server`,
so it dies with `subsystem request failed on channel 0`. Pipe through `ssh` as
above, or use `scp -O` to force the legacy protocol. Both verified byte-for-byte.

The dropbear also races its pty handshake under `sshpass` about one connection in
three and fails with "Permission denied". Just retry, or open one `ControlMaster`
session and reuse it.
:::

A serial console on `ttyS0` at 115200 sees GRUB itself if it doesn't come back.

## IO Extender V1

Bring-up is RAM-only: the stock U-Boot's `run tst` DHCPs, TFTPs a kernel and boots
it without touching flash. There's no persistent install path yet.

```sh
make image BOARD=ioxv1
# then serve it; the board fetches hammer/uImage from ${serverip}:69
```

The armed command is `run tst; run oldbootcmd` with the original saved.

To boot it over the main LAN with no point-to-point adapter and no U-Boot prompt,
see [the LAN TFTP-answer trick](/iox/#booting-it-with-no-serial-console-at-all).

## Before the first boot on any board that has never booted

**Attach a serial console.** Every one of the four proven boards produced at least
one failure that only appears on silicon: a cache that needed flushing, an
interrupt domain that was never created, a file-operation flag that only matters
on `open()`, a pin muxed to a peripheral that isn't on the board.

None of those were visible from a build.
