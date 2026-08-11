# Getting into the CEFDK bootloader

For a long time this was the scary unknown: can you get a prompt out of the
bootloader, or is the EA1 a sealed box that only ever runs what Control4 signed?
The answer, verified end to end on real hardware: **you can get an unlocked CEFDK
shell, and you can boot an unsigned, self-built kernel — through `bootlinux`,
which does no verification at all.** A modern mainline **Linux 7.1.8** + our
Buildroot rootfs now netboots this way, comes up on Wi-Fi, and runs SSH. No fuse
blown, no password cracked, no SPI programmer, no flash written, and the factory
restore button still works.

The catch that took a while to find: the *other* boot paths (`bootkernel`, and
the mfg auto-boot) **do** enforce an RSA signature and we cannot sign. So the
whole trick is to use `bootlinux` and nothing else. Here is exactly how.

## The two buttons

The EA1 has two buttons, and they do completely different things. CEFDK reads
both as GPIOs at power-on (from the GPL source, `brd_gen5/user_init.c`):

```c
#define C4_BUTTON_RECOVERY 31   // the recessed "factory restore" button
#define C4_BUTTON_ID       32   // the ID button on the back
```

* **Recovery button (GPIO 31)** → CEFDK boots the *recovery kernel* straight
  from eMMC (`Execute Control4 Recovery Kernel`, cmdline `... recovery=1`). It
  runs a factory kernel that mounts **p2** and lays the stock rootfs back over
  `/dev/mmcblk0p1` — a factory restore. It restores files from p2; it does not
  repartition (no `mkfs` in the trace).

  We deliberately keep this working. It is the guaranteed one-press return to
  stock Control4, and it stays guaranteed because netboot touches no flash at
  all. Any on-flash install must stay rootfs-only (p1) and never touch p2, the
  recovery kernel, or SPI-NOR/CEFDK. Preserving that escape hatch is a hard
  constraint on anything that ever writes flash.

* **ID button (GPIO 32)** → CEFDK enters *manufacturing mode*, brings up the
  Ethernet, and does a BOOTP request. This is the netboot path, and the way in.

You cannot tell them apart from the banner alone — recovery prints `Factory
Restore (Button): Enabled`, ID prints `Manufacturing Mode: Enabled`. Hold the
wrong one and you get the recovery kernel and wrongly conclude the button does
not netboot. Hold the ID button.

## Getting the shell

With the ID button held, CEFDK prints `Manufacturing Mode: Enabled`, brings the
link up, and BOOTPs up to 12 times. For each reply it checks DHCP **option 60**
against the string `"C4_COOKIE"`. A reply that satisfies it needs: the RFC1048
magic cookie, **option 60 = `"C4_COOKIE"`** (with a trailing NUL — it copies a
fixed 10 bytes and `strcmp`s), a `siaddr` (TFTP server), and a `file` name.

The way in is the failure handling. Every failure *after* the cookie matches
calls `shell(0, NULL)` — the **unlocked** shell, not the SHA-256 password-gated
`shellOnPassword()`:

```c
if (tftpDownload(...) == 0) {
    if (hndBootKernel(4, mfgArgs) != CMD_HANDLED)
        shell(0, NULL);      // boot/verify failed -> shell
} else
    shell(0, NULL);          // tftp failed -> shell
```

So: answer with the cookie but make the TFTP auto-fetch **fail**, and CEFDK
prints `Failed to tftp manufacturing Kernel. Dropping to shell.` and hands you
`shell>` with no password. `tools/netboot.py probe` does exactly this (cookie,
bogus bootfile), and our `tools/tftp-serve.py` errors the auto-fetch of that
bootfile while still serving the real kernel to the manual `tftp get` you type
at the shell.

## The gotcha that wastes an afternoon: link timing

On a **direct** cable between controller and laptop this fails in a way that
looks like the cookie is wrong: CEFDK prints `Bootp configuration failed`, falls
back to a normal boot, and the server logs nothing from CEFDK.

The cause is gigabit autonegotiation. When the controller powers on both ends
renegotiate, and a USB-Ethernet adapter can take longer than CEFDK's whole
12-retry BOOTP window to start forwarding, so the requests never arrive. Fixes:

* **A switch between the two** keeps the laptop↔switch link up permanently, so it
  forwards CEFDK's BOOTP the instant the controller negotiates.
* **No switch: warm-reboot instead of power-cycling.** Log in over serial, hold
  ID, `reboot`. A warm reboot resets the SoC without dropping the PHY, so the
  link never goes down.

macOS wrinkle in the reply: send the BOOTP reply to the subnet-directed
broadcast (`192.168.1.255`), not the limited `255.255.255.255`, or on a
multi-homed Mac it egresses the wrong interface. `netboot.py` does this, and
binds its TFTP sockets to `0.0.0.0` (a specific-IP bind can fail on macOS).

## Two boot commands, one of them verifies

| Command | Verifies? | Load address |
|---|---|---|
| `bootkernel` (mfg / `-id` / `-b <addr>`) | **yes** — `isAuthNeeded()` → RSA `verifyStage3Common` | flag-selectable, but verifies |
| `bootlinux "<cmdline>"` | **no** — only checks `0xAA55` + `HdrS` | global `linuxKernelBase` at `0x000c90a4` |

`isAuthNeeded()` returns true when the SEC_BOOT fuse (`ord4 0xdf8f0060` bit 0,
which reads set and is read-only) **or** the SEC_BOOT strap is set. The verify is
a real RSA-signature check against a key baked into CEFDK; on failure it
`SOFT_HANG`s (`jmp .`). We do not have the private key, so anything through
`bootkernel` or the mfg auto-boot is a dead end (`VERIFY_S3: FAIL` → silent
hang). Do not use them for a custom kernel.

`bootlinux` is a completely separate path. Disassembled, its handler
(`0x81ba3a`) calls a boot routine (`0x812f07`) that is a plain Linux bzImage
loader: it checks the boot magic `0xAA55` at `+0x1fe` and `"HdrS"` at `+0x202`,
copies the setup to `0x40000`, the protected-mode kernel to `0x100000`, the
cmdline to `0x48000`, and far-jumps to the 16-bit entry. There is no RSA, no
hash, no fuse read anywhere in it. An unsigned bzImage boots.

The load address comes from a global, `linuxKernelBase`, at runtime address
**`0x000c90a4`** — plain writable RAM. (CEFDK's image is linked at `0x7d0000`
but its writable data is relocated down to `~0xc9000` at runtime; `0x7d90a4` is a
same-valued lookalike in a boot-descriptor table and is *not* the global.) An
optional ramdisk comes from three more globals: `0x837560` (present flag),
`0x837564` (address — page-aligned means "use in place"), `0x837568` (size).

## The recipe

1. **Shell:** `sudo python3 tools/netboot.py probe --iface-ip 192.168.1.5`, then
   power-cycle holding **ID**. It drops to `shell>` (with an IP already set by
   the BOOTP).
2. **Serve the files:** `sudo python3 tools/tftp-serve.py output/images
   --ip 192.168.1.5`.
3. **Boot it** — `tools/ohc-bootlinux.py` automates the shell side:

   ```
   tftp get 192.168.1.5 0x06000000 bzImage          # kernel into scratch RAM
   tftp get 192.168.1.5 0x04000000 rootfs.cpio.gz   # initrd, page-aligned
   ord4 0x000c90a4 = 0x06000000                      # linuxKernelBase -> kernel
   ord4 0x00837560 = 1                               # ramdisk present
   ord4 0x00837564 = 0x04000000                      # ramdisk addr (in place)
   ord4 0x00837568 = <initrd size>                   # ramdisk size
   bootlinux "console=ttyS0,115200 pci=realloc,nocrs ..."
   ```

Run it with `--server 192.168.1.5 --console-baud 921600` (see UART note below).
It verifies the bzImage/gzip magics actually landed before it commits to the
boot. Everything is RAM-only; a power-cycle returns you to stock.

Two tooling notes that cost real time, now baked into `tftp-serve.py`: CEFDK
asks for a **47040-byte TFTP block**, which macOS can't send in one datagram and
CEFDK can't reassemble — cap `blksize` to 1468. And serve each transfer in a
**thread, never `os.fork()`** — fork is fork-unsafe on macOS and crashed the
server during the mfg request storm.

## Building a kernel that actually fits

`bootlinux` copies the protected-mode kernel to `0x100000`, and its own loader
code lives at `~0x812f07` (≈8 MB) with the final jump near `0x8130be`. If the
copy reaches that, it overwrites the loader mid-copy and crashes on return. So
**the protected-mode kernel must stay under ~7.0 MB** (`0x813000 - 0x100000`).

The stock x86 `i386_defconfig` is a "support every PC ever made" config — a
kernel-only image busts that budget on its own. The fix is the openSpeakerPoint
approach: keep the initramfs **separate** (not embedded), and trim hard. In
`board/ea1/linux/linux.fragment`:

* `CONFIG_KERNEL_XZ`, `CONFIG_CC_OPTIMIZE_FOR_SIZE`
* `CONFIG_MODULES=n` — all-builtin, self-contained. Also dodges an i386-7.1.8
  modpost failure where the musl toolchain compiles `.ko` files with the stack
  protector but `__stack_chk_guard` isn't exported to modules.
* `CONFIG_STACKPROTECTOR=n` — else the toolchain's SSP canary disagrees with
  i386 SMP setup and `__stack_chk_fail` panics in `do_idle` on the secondary CPU.
* `CONFIG_KALLSYMS=n`, and drop the VM-guest / debug / storage / graphics / EFI /
  netfilter / IPv6 dead weight.

Needed drivers are forced `=y` (ath9k, e1000, cfg80211, mac80211, 8250). Result:
**7.1.8 in 4.82 MB**, ~2.2 MB under the ceiling.

## Two board quirks you will hit

* **UART is at 921600, not 115200.** The CE5310's legacy `0x3f8` UART clock is
  **8× standard** (14.7456 MHz), so a mainline kernel asked for 115200 drives the
  wire at `8 × 115200 = 921600`. CEFDK itself knows the real clock and prints
  fine at 115200; the kernel does not. Read the kernel console at 921600 (output
  is clean; typing back is overrun-prone). Proper fix later: set the legacy
  `uartclk`, or the `console=ttyS0,14400` + getty-14400 trick (divisor 8 → true
  115200 on the wire).

* **PCI BARs come up unassigned.** CEFDK only programs the BARs for what it uses,
  so the kernel finds `ath9k`/`e1000` with BAR 0 = `0x00000000` and can't
  ioremap them — no `wlan0`, no `eth0`. Boot with **`pci=realloc,nocrs`** (baked
  into `CONFIG_CMDLINE` + `CMDLINE_EXTEND`) and the kernel assigns them itself.

## Recovery nets, all proven on hardware

| Net | Trigger | Result |
|---|---|---|
| Recovery kernel | recovery button at power-on | factory kernel from eMMC + p2, factory restore |
| initramfs shell | type `c4` in the 2 s window | BusyBox before rootfs mounts |
| Watchdog | any hang/panic | CEFDK's 300 s WDT reboots the box |

`exit` from the initramfs shell **panics the kernel** (it kills PID 1); run
`/normal_boot` to continue booting instead. A confused CEFDK state after
`bootkernel -b` experiments is cleared by a plain power-cycle with no button.

## The CEFDK shell toolbox

```
bootlinux  - boot an unsigned bzImage from linuxKernelBase (NO verify)  <-- ours
bootkernel - boot from flash/memory, VERIFIES the signature (dead end for us)
tftp       - tftp get <server> <ram-addr> <file>   (put/upload is disabled)
ip / ifset - static IP / select interface
ord[2|4]   - read/write memory: ord4 <addr> [= <val>] [len <n>]
mmap       - system memory map (usable RAM is 1-200MB; the rest is reserved DRAM)
strap      - SoC strappings (SB=SEC_BOOT, BP=BOOT_PATH, ...)
spi_flash / emmc / mfh - SPI-NOR / eMMC / flash-header access
settings   - full-screen BIOS editor (ESC to leave; F2 is Upgrade Firmware)
```

## Tools

* `tools/netboot.py` — BOOTP+TFTP server: `observe` / `probe` / `serve` /
  `tftpd` / `shellboot`. Use `probe` for shell entry.
* `tools/tftp-serve.py` — threaded read-only TFTP server for the manual
  `tftp get` (blksize-capped, mfg-storm-proof).
* `tools/ohc-bootlinux.py` — drives the whole boot over serial: stages the
  kernel + initrd, sets the globals, `bootlinux`, and streams the console.
* `tools/serial-console.py` — one-shot serial console, 115200 8N1.

The physical serial header is a 4-pin block near the PSU: **TX / GND / RX /
+3.3 V** (right to left), 3.3 V TTL, 115200.
