---
title: Bootloader access
description: The C4_COOKIE manufacturing-mode protocol, the unlocked CEFDK shell, and booting an unsigned kernel.
sidebar:
  order: 5
---

**The core how-to for the EA family.** Verified end to end on real hardware: you
can get an unlocked CEFDK shell, and you can boot an unsigned, self-built kernel
through `bootlinux`, which does no verification at all.

No fuse blown, no password cracked, no SPI programmer, no flash written, and the
factory restore button still works.

The catch that took a while to find: the *other* boot paths (`bootkernel`, and
the manufacturing auto-boot) **do** enforce an RSA signature and we can't sign.
The whole trick is to use `bootlinux` and nothing else.

## The two buttons

CEFDK reads both as GPIOs at power-on:

```c
#define C4_BUTTON_RECOVERY 31   // the recessed "factory restore" button
#define C4_BUTTON_ID       32   // the ID button on the back
```

- **Recovery button (GPIO 31)** → boots the *recovery kernel* from eMMC
  (`Execute Control4 Recovery Kernel`, cmdline `... recovery=1`) and performs a
  factory restore. We deliberately keep this working: it is the guaranteed
  one-press return to stock, and it stays guaranteed because netboot touches no
  flash. Any on-flash install must never touch p2, the recovery kernel, or
  SPI-NOR.
- **ID button (GPIO 32)** → *manufacturing mode*: brings up Ethernet and does a
  BOOTP request. This is the way in.

You can't tell them apart from the banner at a glance, recovery prints
`Factory Restore (Button): Enabled`, ID prints `Manufacturing Mode: Enabled`.
**Hold the wrong one and you get the recovery kernel** and wrongly conclude the
button doesn't netboot.

## Getting the shell

With the ID button held, CEFDK BOOTPs up to 12 times. For each reply it checks
DHCP **option 60** against `"C4_COOKIE"`. A satisfying reply needs the RFC1048
magic cookie, **option 60 = `"C4_COOKIE"`** with a trailing NUL (it copies a fixed
10 bytes and `strcmp`s), a `siaddr` naming a TFTP server, and a `file`.

The way in is the failure handling. Every failure *after* the cookie matches
calls `shell(0, NULL)` — the **unlocked** shell, not the password-gated
`shellOnPassword()`:

```c
if (tftpDownload(...) == 0) {
    if (hndBootKernel(4, mfgArgs) != CMD_HANDLED)
        shell(0, NULL);      // boot/verify failed -> shell
} else
    shell(0, NULL);          // tftp failed -> shell
```

So: answer with the cookie, but make the TFTP auto-fetch **fail**. CEFDK prints
`Failed to tftp manufacturing Kernel. Dropping to shell.` and hands over `shell>`
with no password. The tooling does this by naming a bootfile that doesn't exist,
while still serving the real kernel to the manual `tftp get` typed at the shell.

## The gotcha that wastes an afternoon: link timing

On a **direct** cable this fails in a way that looks like a wrong cookie: CEFDK
prints `Bootp configuration failed`, falls back to normal boot, and the server
logs nothing.

The cause is gigabit autonegotiation. When the controller powers on both ends
renegotiate, and a USB-Ethernet adapter can take longer than CEFDK's whole
12-retry BOOTP window to start forwarding, so the requests never arrive.

- **Put a switch between them**, the laptop↔switch link stays up, so it forwards
  CEFDK's BOOTP the instant the controller negotiates.
- **Or warm-reboot instead of power-cycling.** Log in over serial, hold ID,
  `reboot`. A warm reboot resets the SoC without dropping the PHY.

Two macOS wrinkles: send the BOOTP reply to the **subnet-directed broadcast**
(`192.168.1.255`), not the limited `255.255.255.255`, or on a multi-homed Mac it
egresses the wrong interface; and bind the TFTP sockets to `0.0.0.0`, because a
specific-IP bind can fail there.

## Two boot commands, one of them verifies

| Command | Verifies? | Load address |
|---|---|---|
| `bootkernel` (mfg / `-id` / `-b <addr>`) | **yes** — `isAuthNeeded()` → RSA `verifyStage3Common` | flag-selectable, but verifies |
| `bootlinux "<cmdline>"` | **no** — only checks `0xAA55` + `HdrS` | global `linuxKernelBase` at `0x000c90a4` |

`isAuthNeeded()` returns true when the SEC_BOOT fuse **or** the SEC_BOOT strap is
set. The verify is a real RSA check against a key baked into CEFDK; on failure it
`SOFT_HANG`s (`jmp .`). Control4 swapped Intel's key for their own, so signing is
not an option. **Do not use `bootkernel` for a custom kernel.**

`bootlinux` is a completely separate path. Disassembled, its handler (`0x81ba3a`)
calls a boot routine (`0x812f07`) that's a plain Linux bzImage loader: check the
boot magic `0xAA55` at `+0x1fe` and `"HdrS"` at `+0x202`, copy the setup to
`0x40000`, the protected-mode kernel to `0x100000`, the cmdline to `0x48000`, and
far-jump to the 16-bit entry. **No RSA, no hash, no fuse read anywhere in it.**

:::caution[The load-address global has a lookalike]
`linuxKernelBase` is at runtime address **`0x000c90a4`**, plain writable RAM.
CEFDK's image is linked at `0x7d0000` but its writable data is relocated down to
about `0xc9000` at runtime, and `0x7d90a4` is a **same-valued lookalike** in a
boot-descriptor table. It isn't the global. Writing to it does nothing and
reports nothing.
:::

An optional ramdisk comes from three more globals: `0x837560` (present flag),
`0x837564` (address — page-aligned means "use in place"), `0x837568` (size).

## The recipe

```sh
make image BOARD=ea3-v2      # Buildroot, in Docker
make netboot BOARD=ea3-v2    # BOOTP + TFTP + drives the shell over serial
```

`make netboot` does the whole thing in one process:

1. threaded TFTP over `output/images` (both `bzImage` and `rootfs.cpio.gz`);
2. BOOTP replies carrying the `C4_COOKIE` but naming a bootfile that doesn't
   exist, so the auto-fetch fails and CEFDK drops to the **unlocked** shell;
3. it watches the serial console and, the moment `shell>` appears, drives the
   sequence itself;
4. reopens the console at the kernel's real baud and streams the boot log.

What it types, for reference:

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
and **refuses to boot if any of them didn't stick**, a bad stage should be a
clear error, not a silent hang.

Everything is RAM-only; a power-cycle returns you to stock.

The only manual step is holding the **ID button** while the unit restarts.
Nothing in software can press it, so the tool prints a reminder and waits.
`sudo` is unavoidable — BOOTP and TFTP are privileged ports (67/69).

Per-board addressing lives in one board table. **The MAC matters more than it
looks:** the responder answers only that MAC, which is what keeps it safe to run
on a live network alongside a real DHCP server, so a wrong one means every
request is silently ignored and the console just says
`Bootp configuration failed`.

Two tooling notes that cost real time: CEFDK asks for a **47040-byte TFTP block**,
which macOS can't send in one datagram and CEFDK cannot reassemble, so cap
`blksize` to 1468. And serve each transfer in a **thread, never `os.fork()`** —
fork is fork-unsafe on macOS and crashed the server during the manufacturing
request storm.

## The CEFDK shell toolbox

```
bootlinux  - boot an unsigned bzImage from linuxKernelBase (NO verify)  <-- ours
bootkernel - boot from flash/memory, VERIFIES the signature (dead end)
tftp       - tftp get <server> <ram-addr> <file>   (put/upload is disabled)
ip / ifset - static IP / select interface
ord[2|4]   - read/write memory: ord4 <addr> [= <val>] [len <n>]
cache      - cache flush (REQUIRED after `emmc rd`, see secure boot)
emmc       - rd/wr raw, rd_bp/wr_bp boot partitions, set_boot, info, dump csd
spi_flash  - rd/wr the NOR, including CEFDK itself
mfh        - manage NOR slots (Master Flash Header)
script     - store/enable an autorun script  <-- the EA3 takeover
mmap       - system memory map (usable RAM is 1-200MB; the rest is reserved)
strap      - SoC strappings (SB=SEC_BOOT, BP=BOOT_PATH, ...)
ymodem     - serial file transfer
settings   - full-screen BIOS editor (ESC to leave; F2 is Upgrade Firmware)
```

The shipping shell is a reduced set — there is no `msr` and no `mmc` — so run
`help` rather than assuming a command from the source is present.

## The serial header

A 4-pin block near the PSU: **TX / GND / RX / +3.3 V** (right to left), 3.3 V
TTL, 115200.
