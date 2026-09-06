---
title: The bootloader with no prompt
topic: Bootloader
summary: Every key at every baud through the whole CEFDK window got nothing back. The shell was there the whole time, behind a button I wasn't pressing and a password hash I don't have.
description: Why CEFDK cannot be interrupted on a normal boot, and what the GPL source says about it.
sidebar:
  order: 2
  label: The bootloader with no prompt
---

CEFDK announces itself on `ttyS0` at 115200 and tells you what it is before doing
anything else:

```
CEFDK Version : CE5300 (SMP enabled)     Boot Mode : SPI-NOR (STRAPS)
Board         : Type 1, Rev 5            MAC       : 00:0f:ff:1a:fc:a9
8051 Firmware : C0-1.0.53                Silicon   : D0 (PCI), SKU 0x08F
```

Then it prints `Executing Control4 Normal Boot Mode` and it's gone. No pause, no
countdown, no "press any key".

## The obvious thing, done thoroughly, does nothing

Ctrl-C, ESC, space, CR, `x`, the literal string `c4`, all sent continuously at
115200 through the entire CEFDK window, across a reboot. Nothing came back. No
prompt, no delay, no acknowledgement of any kind.

It would have been easy to write that up as "CEFDK has no shell" and move on to
replacing userspace instead. That would have been wrong, and the GPL drop says so
plainly. `brd_gen5/user_init.c` has a break-in prompt. It's just gated twice.

## Gate one: it's only offered in manufacturing mode

`userInit()` calls the break-in function only after `c4_id_button_is_pressed()`
reports manufacturing mode. That means the recovery button held at power-on, or a
forced cookie. No button, no prompt, which is exactly the behaviour I was seeing.

So the first correction had nothing to do with cryptography. I'd been typing into
a window that was never opened.

## Gate two: and when it is offered, it wants a password

Older CEFDK builds (`shellOnC4`) took the literal string `c4`. The build on these
units is `shellOnPassword`, from Control4 patch
`0035-use-password-to-enter-shell`, and it swaps that for a SHA-256 check through
`trusted_boot_sec_library`. It reads a line, hashes it, compares against a 32-byte
digest baked into the image starting `ec 89 70 13 ad f9 …`.

I don't have the preimage and it isn't in the drop.

There's a small irritating detail here. The CA-1's U-Boot uses the *same hash*.
Control4's `CONFIG_KEYED_BOOTDELAY` build prints no "Hit any key" prompt, reads a
line, SHA-256's it, and compares against the same digest. One password, two
completely different bootloaders, two different SoCs. Whoever finds that preimage
opens two doors at once.

## So the accurate statement is narrower than "sealed"

On a normal boot CEFDK can't be interrupted, and the manufacturing-mode shell it
does have is password-locked with a hash nobody outside Control4 holds.

At the time that was the same as "no shell" for practical purposes. But it's a
locked door rather than a missing one, and the distinction ended up mattering
enormously, because [the way in](/log/answering-with-a-cookie/) doesn't
involve the password at all. It involves the error handling.

## Meanwhile, the escape hatch that does exist

There is a serial break-in on these units. It just isn't in the bootloader. It's
inside the kernel's initramfs:

```
Type 'c4' followed by [ENTER] within the next 2 seconds to stop boot and
break into initramfs.
```

Type `c4` and you get a BusyBox root shell before the rootfs is mounted, with the
eMMC reachable as `/dev/mmcblk0` and `dd`, `mount`, `tar` and `vi` available.
Leaving the shell continues the boot normally, so poking around costs nothing.

That produces an asymmetry which governed the order of everything afterwards:

- a broken **rootfs** is recoverable over serial, because the initramfs still runs;
- a broken **kernel** is not, because the initramfs providing the escape is part
  of the kernel image that failed to load.

Recovering from a bad kernel would need external hardware, a clip onto the flash.
So the sequencing rule wrote itself. Prove everything in RAM first, and don't
write a kernel to flash until the thing you're writing has already booted.

One footnote that cost a reboot: typing `exit` in that initramfs shell panics the
kernel, because it kills PID 1. Run `/normal_boot` to continue instead.

## The trap next door

While mapping how `/init` picks a boot mode I found a path that looks like
exactly what a netboot experiment wants, and very much isn't.

`/init` reads `/proc/cmdline` and picks one of four paths:

| Condition | Mode |
|---|---|
| `mfgtest` in cmdline | `/mfg_prog` |
| `recovery` in cmdline | `/recovery_boot` |
| no `/dev/mmcblk0p1` **and** no `nfsroot` | `/dev_prog_boot` |
| otherwise | `/normal_boot` |

`/dev_prog_boot` mounts NFS, which is the tempting part. Here's its actual body,
read off the device:

```
format_emmc_8g            <-- wipes the entire 8 GB eMMC
create_filesystems
mount_nfs
update_factory_kernel
extract_rootfs
install_boot_kernel
install_spiflash_kernel   <-- reflashes the SPI-NOR bootloader
reboot -f
```

It's a factory programmer. Running it to "try netboot" formats the eMMC and
reflashes both the kernel and the bootloader from whatever an NFS server happened
to be serving at the time. The name was the warning all along: development
*programmer* boot.

## What did work, safely

Before any of the bootloader work paid off, a plain NFS root proved the principle
end to end on a live EA1. Served from a laptop by a small userspace NFSv3 server,
so no `sudo`, no `/etc/exports`, no privileged ports:

```
$ chroot /tmp/nfstest /bin/busybox sh -c '...'
  chrooted OK
  marker : netboot-root-marker-1785539875
  uname  : 3.12.74
  bin/   : busybox cat echo hostname ls mount ps sh sleep uname
```

Binaries executed using the NFS-served dynamic loader and libc, which is the whole
risk in netbooting a userspace. It didn't prove `switch_root` into it as PID 1 —
that needs someone at the box to power-cycle if it hangs, but it proved the
kernel could mount and run a complete network-served root with zero writes to the
device.

Enough to justify the next step: stop trying to interrupt CEFDK, and start trying
to make it talk to me on the network.
