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

Everything goes over SSH from stock Control4 or a running openHC. Nothing needs
a serial console or the ID button, and the factory-restore partition (`sda2`) is
never written. All four flows below were run on a unit on 2026-10-02.

### Try it from RAM (writes nothing)

```sh
ohc-flash install <host> --images openhc-hc800-<version>.zip            # --method kexec is the default
```

The tool mounts a tmpfs sized to the release at `/mnt/ohc-stage`, copies the
kernel and initramfs into it, and `kexec`s into openHC from whatever is running.
The stock image ships no `kexec`, so the hc800 bundle carries a static i686 one
(`kexec-i686-static`) that the tool pushes when the box has none. A power cycle
returns the box to stock. The release is about 43 MB (13.8 MB kernel + 29.7 MB
initramfs), which no longer fits in the stock image's 32 MB `/tmp`. That's why
the tool stages in its own tmpfs.

### Install to disk

```sh
ohc-flash install <host> --images openhc-hc800-<version>.zip --method grub --boot-once
ohc-flash boot    <host>      # reboot into it
```

This copies the images onto `sda3` (the ext3 kernel partition, ~165 MB free)
and appends a third GRUB entry. Both vendor entries and the `factorydefault`
lines stay byte-identical. GRUB 0.97 loads the full 43 MB image from there.

`--boot-once` sets `default saved`, and openHC's entry runs `savedefault 1`
before it boots. Every openHC boot therefore hands the default straight back to
Control4, so a panic, power cut or reset always lands on stock, which answers
SSH. `ohc-flash boot` re-enters openHC; it changes one byte. Without
`--boot-once`, openHC is the default on every boot.

```sh
ohc-flash uninstall <host>   # put back the as-shipped menu.lst, delete our files from sda3
ohc-flash restore   <host>   # Control4's own factory restore, once, then stock
```

`restore` is the software ID button. It appends a copy of the factory entry
that `savedefault`s back to the stock entry, and points the saved default at
it. The box runs Control4's restore once (about 5 minutes, re-imaging
`sda3`/`sda4` from `sda2`) and reboots into a clean stock image. It never writes
`default 0`: Control4's `restore.sh` reboots without touching `menu.lst`, so
that restores forever. An earlier version of the tool did exactly that.

The first `menu.lst.pre-openhc` the tool writes is the as-shipped file, and it
is never overwritten. Keep a byte copy of `sda1` off the box too. It is the one
partition with no on-disk recovery.

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

### Install to NAND

`ohc-flash` installs openHC over SSH, with no serial console and no button,
from either stock Control4 or a running openHC (method `nand`):

```sh
ohc-flash install <host> --images openhc-ioxv1-<version>.zip   # ~2-3 minutes
```

The image goes in a 32 MiB slot at `0x10000000`, the start of the half of the
512 MiB NAND that stock never partitioned. No stock partition, bootloader or
recovery image is touched. The tool uploads, checks the md5, erases, writes,
reads the slot back through ECC, and only then changes U-Boot's environment:
`bootcmd=run ohcboot`, plus a boot-attempt counter, `ohc_try`.

Each boot, U-Boot bumps `ohc_try` and saves it before starting openHC, and openHC
clears it once its uplink has carrier and an address. A kernel that panics,
hangs (U-Boot arms the hardware watchdog and openHC feeds it) or never gets a
network leaves the count climbing, and at 3 U-Boot runs the untouched stock boot
instead. That was tested by installing a kernel command line that panics: three
counted attempts, then stock Control4, with nobody touching the box. From stock,
`install` again takes it back.

```sh
ohc-flash boot      <host>   # boot openHC again, e.g. after a fallback
ohc-flash uninstall <host>   # make U-Boot boot stock Control4 again
ohc-flash restore   <host>   # factory: U-Boot's default bootcmd, our variables gone, slot erased
```

### Netboot (development)

The stock U-Boot's `run tst` DHCPs, TFTPs a kernel and boots it from RAM without
touching flash. An install replaces the `bootcmd` that runs it, so to netboot an
installed box, set `bootcmd` back to `run tst; run oldbootcmd` with `fw_setenv`.

The board's `tst` TFTPs `hammer/uImage` from a hardcoded `192.168.0.10`, so the
serving machine has to hold that address. `ohc-flash netboot` answers DHCP for
that one MAC and serves the image; on macOS it runs without `sudo`:

```sh
sudo ifconfig en0 alias 192.168.0.10 255.255.255.0   # once per boot of the Mac
ohc-flash netboot --board ioxv1 --mac 00:0f:ff:xx:xx:xx \
    --image openhc-ioxv1-kernel.img --minutes 600
# power-cycle the IO Extender; it comes up at 192.168.0.50
```

Nothing proprietary has to be supplied: at boot, `S12fpga` copies the FPGA
bitstream read-only from the unit's own recovery rootfs on NAND.

If the LAN router already holds a lease for the box's MAC it usually answers
first, and the board then TFTPs through the gateway and never reaches you.
Delete or block that lease on the router.

To boot it over the main LAN with no point-to-point adapter and no U-Boot prompt,
see [the LAN TFTP-answer trick](/iox/#booting-it-with-no-serial-console-at-all).

## Before the first boot on any board that has never booted

**Attach a serial console.** Every one of the four proven boards produced at least
one failure that only appears on silicon: a cache that needed flushing, an
interrupt domain that was never created, a file-operation flag that only matters
on `open()`, a pin muxed to a peripheral that isn't on the board.

None of those were visible from a build.
