# Boot chain, netboot, and running custom kernels

How the EA1 actually boots, and the three ways to get our own code running on it.
All of this is read off a live unit over the serial console; nothing here is
inference from documentation.

## The chain

```
SPI-NOR ──► Intel CEFDK ──► kernel (raw eMMC) ──► initramfs /init ──► one of four boot modes
```

CEFDK announces itself on `ttyS0` at 115200 and reports the board before doing
anything else:

```
CEFDK Version : CE5300 (SMP enabled)     Boot Mode : SPI-NOR (STRAPS)
Board         : Type 1, Rev 5            MAC       : 00:0f:ff:1a:fc:a9
8051 Firmware : C0-1.0.53                Silicon   : D0 (PCI), SKU 0x08F
```

## CEFDK cannot be interrupted in normal boot — but it *has* a shell

Tested directly: Ctrl-C, ESC, space, CR, `x` and `c4` were sent continuously at
115200 through the entire CEFDK window across a reboot. No prompt, no pause, no
delay — CEFDK goes straight from its banner to `Executing Control4 Normal Boot
Mode`.

The reason is narrower than "no shell", though, and the CEFDK source (the GPL
drop, `brd_gen5/user_init.c`) says so plainly. There *is* a break-in prompt —
it is just gated behind manufacturing mode, which a normally-booting unit is not
in. The gate:

* `userInit()` only calls the break-in function after `c4_id_button_is_pressed()`
  reports manufacturing mode — the recovery button held at power-on, or a forced
  cookie. No button, no prompt, which is exactly what we saw.
* Older CEFDK (`shellOnC4`) accepted the literal string `c4`. The build on our
  unit (`shellOnPassword`, patch `0035-use-password-to-enter-shell`) replaced
  that with a **SHA-256 password check** via `trusted_boot_sec_library`:
  it reads a line, hashes it, and compares against a 32-byte digest baked into
  the image (`ec 89 70 13 ad f9 …`). We do not have the preimage, and it is not
  in the drop.

So the accurate statement is: **on a normal boot CEFDK cannot be interrupted,
and the manufacturing-mode shell it does have is password-locked with a hash we
do not hold.** For our purposes the consequence is the same as before — we
cannot drop to a CEFDK prompt on a production unit — but it is a locked door,
not a missing one, and that distinction matters if a preimage ever surfaces or
the mfg cookie path is explored.

**The escape hatch we actually use lives INSIDE the kernel** (the initramfs `c4`
prompt below). So:

* a broken **rootfs** is recoverable over serial — the initramfs still runs;
* a broken **kernel** is NOT recoverable over serial, because the initramfs that
  provides the escape is part of the kernel image that failed to load. Recovery
  would need external hardware (an eMMC/SPI programmer).

That asymmetry is why the ordering in "Order of attack" below matters, and why
NFS netboot — which touches no flash at all — is the right way to prove custom
code before anything is written.

## Breaking in — the recovery net

The initramfs offers a 2-second escape hatch on the serial console:

```
Type 'c4' followed by [ENTER] within the next 2 seconds to stop boot and break into initramfs.
```

Typing `c4` drops you to a BusyBox root shell **before** the rootfs is mounted:

```
/init failed: Exiting Boot Sequeunce At User Request
ERROR! Dropping to a shell
BusyBox v1.27.1 built-in shell (ash)
~ #
```

This is the recovery path for anything short of a corrupt kernel: eMMC is
reachable as `/dev/mmcblk0` with `dd`, `mount`, `tar` and `vi` available. It is
also non-destructive — leaving the shell continues the normal boot.

## Boot mode selection (all from the kernel command line)

`/init` picks one of four paths, and every decision reads `/proc/cmdline`:

| Condition | Mode |
|---|---|
| `mfgtest` in cmdline | `/mfg_prog` |
| `recovery` in cmdline | `/recovery_boot` |
| no `/dev/mmcblk0p1` **and** no `nfsroot` in cmdline | `/dev_prog_boot` |
| otherwise | `/normal_boot` |

The stock cmdline, set by CEFDK, is:

```
console=ttyS0,115200 rw root=/dev/mmcblk0p1 rootwait ip=none memmap=exactmap
memmap=128K@128K memmap=1585M@1M vmalloc=586M androidboot.hardware=intelce
```

## ⚠ `/dev_prog_boot` IS A FACTORY PROGRAMMER — DO NOT RUN IT

It mounts NFS, but that is the least of what it does. Its main body, read off
the device:

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

Running it to "try netboot" would format the eMMC and reflash both the kernel
and the SPI-NOR from whatever the NFS server happened to be serving. With CEFDK
non-interruptible, a bad or incomplete export would leave the box needing an
external programmer. The name is the warning: *development **programmer** boot*,
reached from "No partitions on eMMC. Enter Manufacturing Mode."

Use the manual path below instead.

## NFS netboot — PROVEN, and it touches no flash

Verified end to end on a live EA1. A read-only NFS root was served from a laptop
by `tools/ohc-nfsd.py` (a small userspace NFSv3 server, so no `sudo`, no
`/etc/exports`, no privileged ports), mounted on the box, and **binaries were
executed from it using the NFS-served dynamic loader and libc**:

```
$ chroot /tmp/nfstest /bin/busybox sh -c '...'
  chrooted OK
  marker : netboot-root-marker-1785539875
  uname  : 3.12.74
  bin/   : busybox cat echo hostname ls mount ps sh sleep uname
```

Reproduce:

```bash
# laptop — serve a root read-only, no root privileges needed
python3 tools/ohc-nfsd.py --root ./nfsroot --nfs-port 2049 --mount-port 2050

# box — mount and run
mount -t nfs -o ro,nolock,vers=3,tcp,port=2049,mountport=2050,soft \
      <laptop-ip>:/ea1 /tmp/nfstest
chroot /tmp/nfstest /bin/busybox sh
```

The NFS root needs the loader and libc alongside busybox (`/lib/ld-linux.so.2`
and `/lib/libc.so.6` — the stock busybox is dynamically linked), and everything
must be mode 755.

**What this does and does not prove.** It proves the kernel can mount an NFS root
and execute a complete network-served userspace, which is the whole risk in
netbooting. It does not yet prove `switch_root` into it as PID 1 — that needs
someone at the device to power-cycle if it hangs, so it is deliberately left for
a session with hands on the box.

### For reference, what Control4's own NFS logic looked like

`/dev_prog_boot` DHCPs on eth0 and mounts:

```sh
DEFAULT_SERVER=10.11.208.100          # used when DHCP gives no serverip
NFS_SERVER_PATH=/nfsroot
mount -t nfs -o ro,nolock ${SERVER}:/nfsroot/$(c4_board_name) /mnt/nfs
```

So the board looks for `<server>:/nfsroot/ea1`. `SERVER` comes from the DHCP
`serverip` option, falling back to `10.11.208.100`.

This is by far the best development loop: **an entire custom userspace with zero
writes to the device.** A bad image costs a power cycle, not a recovery session.
It needs an NFS export named after the board and a DHCP server handing out
`serverip` (or the box on 10.11.208.0/24 with that host serving).

## Option 2 — replace the userspace (what we do today)

A lighter alternative to a custom kernel: keep the stock kernel and graphics
stack and replace only the userspace on p1. Reversible, and independent of this
repo — but it inherits the vendor kernel. A kernel-up build (netboot, below) is
what this project is about.

## Option 3 — a custom kernel

CEFDK does **not** read the kernel from a filesystem. It reads it from a raw
offset near the start of the eMMC, which the boot log states outright:

```
Read Kernel Size Successfully from emmc address(0x00000200)! Kernel is (7009216)(0x006af3c0) bytes
Successfully read (7009216) bytes of kernel into memory at (0x00f00000) from emmc address (0x00000400).
VERIFY_S3(kernel bzImage): PASS
```

Verified byte-for-byte on the device:

```
eMMC 0x200  u32 BE   container size          -> 00 6a f3 c0  = 7,009,216  (matches the log)
eMMC 0x400  CEFDK container header, 0x580 bytes
              +0x10  0x8086                  Intel vendor id
              +0x14  25 05 22 20             build date, matches CEFDK's 05/25/22
              +0x28  0x00000580              offset from container start to the bzImage
eMMC 0x980  bzImage                          "HdrS" lands at 0x980+0x202 = 0xb82 ✓
```

So the recipe for a custom kernel is: build a `bzImage`, prepend the 0x580-byte
container header (copy the existing one and patch the length fields), write it at
raw offset `0x400`, and write the new total size as a **big-endian u32** at
`0x200`.

### Is the signature enforced?

Reading `SEC_BOOT_FUSE` from the DFX unit via `/dev/mem` says no — see
[GPL source](gpl-source.md) for the register dump:

```
VERDICT: signature checking NOT enforced by the fuse -> a self-built kernel should boot.
         (STRAP bit not readable here; if the box still refuses, the board strap
         is the remaining suspect.)
```

So `VERIFY_S3 ... PASS` is expected to be advisory on this unit. That is a
prediction, not yet a demonstration — **nobody has booted a self-built kernel on
this box yet.** Do not attempt it without the serial console attached, because a
kernel CEFDK refuses to load leaves nothing to break into: the `c4` escape lives
in the initramfs, which is inside the very kernel that failed.

### Order of attack, safest first

1. **NFS netboot** — proves custom userspace end to end, risks nothing.
2. **Custom kernel written to the *recovery* slot first** (p2 is 1 GB and the
   `recovery` cmdline path exists), so the normal slot stays bootable.
3. Only then the primary kernel slot.

Back up `0x200`–`0x980` and the whole kernel region before writing either:

```sh
dd if=/dev/mmcblk0 bs=512 count=14000 of=/mnt/backup/kernel-slot.bin
```
