---
title: Answering with a cookie
topic: Bootloader
summary: The way into an unlocked shell was the failure handler. Answer the BOOTP correctly, then make the download fail on purpose.
description: The C4_COOKIE manufacturing-mode protocol, the unlocked CEFDK shell, and the link-timing trap.
sidebar:
  order: 3
  label: Answering with a cookie
---

CEFDK's manufacturing mode brings up Ethernet and BOOTPs for a kernel. If it
doesn't like the answer, it declines with a message that reads like a closed
door:

> Control4 Manufacturing Mode Network Cookie Not Found. Continue Normal Boot.
> Button may be depressed accidentally or user may be doing a network reset.

The cookie is `C4_COOKIE`, in DHCP option 60, and the source doesn't hide it:
`const char* C4_MFG_COOKIE = "C4_COOKIE";` from `0007-control4-bootflow.patch`. A
reply CEFDK accepts needs the RFC1048 magic cookie, option 60 set to that string
*with* its trailing NUL (it copies a fixed ten bytes and `strcmp`s), a `siaddr`
naming a TFTP server, and a `file`.

## Two buttons, doing opposite things

Before any of that worked, one thing had to be right, and getting it wrong gives
you a convincing false conclusion. CEFDK reads two GPIOs at power-on:

```c
#define C4_BUTTON_RECOVERY 31   // the recessed "factory restore" button
#define C4_BUTTON_ID       32   // the ID button on the back
```

The recovery button boots the recovery kernel from eMMC and does a factory
restore. The ID button enters manufacturing mode and netboots.

From the banner they're easy to confuse: one prints
`Factory Restore (Button): Enabled`, the other `Manufacturing Mode: Enabled`.
Hold the wrong one, watch a factory kernel come up, decide the button doesn't
netboot. Hold the ID button.

## The way in is the error path

This is the part that makes the whole project possible. Every failure *after* the
cookie matches calls `shell(0, NULL)`, which is the unlocked shell, not the
SHA-256-gated `shellOnPassword()`:

```c
if (tftpDownload(...) == 0) {
    if (hndBootKernel(4, mfgArgs) != CMD_HANDLED)
        shell(0, NULL);      // boot/verify failed -> shell
} else
    shell(0, NULL);          // tftp failed -> shell
}
```

Answer with the cookie, then make the automatic TFTP fetch fail on purpose. CEFDK
prints `Failed to tftp manufacturing Kernel. Dropping to shell.` and hands over
`shell>` with no password.

That's the whole trick. Nobody attacked the password, blew a fuse, or touched an
SPI programmer. You satisfy the protocol far enough to be trusted, then fail in
the specific way that drops to a prompt.

The tooling does it by naming a bootfile that doesn't exist, while still serving
the real kernel to the manual `tftp get` you type at the shell.

## The afternoon that went to link timing

On a direct cable between controller and laptop this fails in a way that looks
exactly like a wrong cookie. CEFDK prints `Bootp configuration failed`, falls back
to a normal boot, and the server logs nothing at all. Nothing arrived.

It's gigabit autonegotiation. When the controller powers on both ends
renegotiate, and a USB-Ethernet adapter can take longer to start forwarding than
CEFDK's entire twelve-retry BOOTP window. The requests go into a link that isn't
up yet.

Two fixes, both reliable:

- **Put a switch between them.** The laptop-to-switch link stays up permanently,
  so the switch forwards CEFDK's BOOTP the instant the controller negotiates.
- **Warm-reboot instead of power-cycling.** Log in over serial, hold ID, `reboot`.
  A warm reboot resets the SoC without dropping the PHY, so the link never goes
  down.

A macOS wrinkle in the same area: send the BOOTP reply to the subnet-directed
broadcast (`192.168.1.255`), not the limited `255.255.255.255`, or on a
multi-homed Mac it leaves by the wrong interface. Bind the TFTP sockets to
`0.0.0.0` too; a specific-IP bind can fail there.

Two more that cost real time and are now baked into the tooling. CEFDK asks for a
47040-byte TFTP block, which macOS can't send in one datagram and CEFDK can't
reassemble, so `blksize` gets capped at 1468. And each transfer has to be served
in a thread, never `os.fork()`, fork is fork-unsafe on macOS and crashed the
server during the manufacturing request storm.

## Two boot commands, only one of them useful

At the shell there are two ways to boot, and the difference between them is the
difference between a project and a dead end.

| Command | Verifies? | Notes |
|---|---|---|
| `bootkernel` | **yes** — `isAuthNeeded()` → RSA `verifyStage3Common` | `SOFT_HANG`s on failure. Dead end. |
| `bootlinux "<cmdline>"` | **no** — only `0xAA55` + `HdrS` | loads from a global at `0x000c90a4` |

Disassembled, `bootlinux`'s handler calls a routine that's a plain Linux bzImage
loader. Check the boot magic at `+0x1fe` and `"HdrS"` at `+0x202`, copy the setup
to `0x40000`, the protected-mode kernel to `0x100000`, the cmdline to `0x48000`,
far-jump to the 16-bit entry. There's no RSA, no hash and no fuse read anywhere in
it.

The load address comes from a global, `linuxKernelBase`, at runtime address
`0x000c90a4`. Plain writable RAM. Pinning that down took some care: CEFDK's image
is linked at `0x7d0000` but its writable data relocates down to around `0xc9000`
at runtime, and `0x7d90a4` is a same-valued lookalike sitting in a boot-descriptor
table. Writing to the lookalike does nothing and reports nothing.

An optional ramdisk comes from three more globals: `0x837560` for the present
flag, `0x837564` for the address (page-aligned means "use in place"), `0x837568`
for the size.

## What the tool types

The whole sequence, which `make netboot` now drives automatically after watching
for `shell>` on the console:

```
tftp get <server> 0x06000000 bzImage          # kernel into scratch RAM
tftp get <server> 0x04000000 rootfs.cpio.gz   # initrd, page-aligned
ord4 0x000c90a4 = 0x06000000                  # linuxKernelBase -> kernel
ord4 0x00837560 = 1                           # ramdisk present
ord4 0x00837564 = 0x04000000                  # ramdisk addr (in place)
ord4 0x00837568 = <initrd size>               # ramdisk size
bootlinux "console=ttyS0,115200 pci=realloc,nocrs ..."
```

It reads back the bzImage `0xAA55` and gzip `0x1f8b` magics and all four globals,
and refuses to boot if any of them didn't stick. A bad stage should be a clear
error, not a silent hang, which is what a wrong `linuxKernelBase` gives you.

All of it's RAM-only. A power cycle returns the unit to stock Control4, which is
the only reason it was reasonable to iterate at all.

The one thing no software can do is hold the ID button, so the tool prints a
reminder and waits.

## Where that left things

An unlocked bootloader shell, an unverified boot command, and a development loop
that writes nothing. That's a complete answer to "is this a sealed box".

What it isn't is a kernel that boots. The first serious attempt hung, and the
reason turned out to be a number nobody had thought to check.
