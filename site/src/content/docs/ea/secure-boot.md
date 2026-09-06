---
title: Secure boot and the takeover
description: The EA3's blown fuse, the measurement that read the wrong register, and the CEFDK autoscript that makes it moot.
sidebar:
  order: 6
---

CEFDK implements a two-stage signature chain keyed to Control4:
`s2pubkey-c4.h` carries a Control4 RSA-2048 modulus for stage1→stage2, and patch
`0010-enable-kernel-authentication` sets `CFG_KERNEL_AUTHENTICATION=1` and swaps
in Control4's key for stage2→kernel. No private keys are in the GPL drop, and the
signing tools ship as binaries without source. **Signing our way out isn't
available.**

Enforcement is hardware-gated, and this is where it gets interesting.

## The measurement that was correct and useless

The gate, per `brd_gen5/sec_boot_linux.c` in the GPL drop:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x14) & (2<<21)) ||  // SEC_BOOT_FUSE
    cp_strap_sts_0().strap.sec_boot)                        // SEC_BOOT_STRAP
```

That register is readable from a running Linux, because the stock kernel has
`CONFIG_STRICT_DEVMEM` unset. Reproducing the check against `/dev/mem` on an EA1
gave:

```
DFX device      : 0000:01:0b.7      (AV_BUS:0b.7, per CEFDK's pciR32)
BAR0            : 0xdf8f0000
reg[BAR0+0x14]  : 0x00000000
SEC_BOOT_FUSE   : 000 (bits 23:21)
secure-boot fuse: CLEAR
```

Conclusion at the time: signature enforcement is off, a self-built unsigned
kernel should boot. And on the EA1 it does.

:::danger[Correction — this reads the wrong register]
`dfx_mbar + 0x14` is the **pre-patch Intel** location. Control4's CEFDK patch
`0022-s3-boot-verify-fix` **moved the check** to `dfx_mbar + 0x60` bit 0 and
dropped the `rev_id == 4` escape hatch:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x60) & BIT(0)) ||  /* SEC_BOOT_FUSE=1 */
    cp_strap_sts_0().strap.sec_boot)
    return true;                                            /* enforced */
```

So `+0x14 == 0` says nothing about the shipping check. We validated against the
upstream source when the shipping binary was patched, and the patch series was
in the same archive we had already downloaded.
:::

## What an EA3 actually does

Proven on hardware on an **EA3 board v2**: writing an openHC kernel into the eMMC
container at `0x400` and doing a normal power-on gives

```
VERIFY_S3(kernel bzImage): FAIL
```

then a `SOFT_HANG` and a watchdog boot loop. The factory-restore button recovers
it fully.

**The fuse is blown on this board.** The EA1 (board **v1**) still boots unsigned
kernels on the normal path, so the fuse likely differs by board revision. That
also means an **EA3 v1 may behave like the EA1** and could take over via the plain
eMMC-container path with no autoscript. Untested.

## The door that was open anyway

The fuse blocks exactly one thing: `hndBootKernel`, which is `bootkernel`, which
is the normal boot path.

Two facts defeat it without touching it:

1. **`bootlinux` doesn't verify.** It drives `bootLinux()`
   (`brd_gen5/boot_linux.c`), which does **zero** signature checking — only the
   `0xAA55`/`HdrS` sanity.
2. **`userInit()` runs `runAutoScript()` before the verifying path**, when
   `g_bios_settings.script == 0`. CEFDK has a stored-script facility as an MFH
   slot type in SPI-NOR, and it executes *first*.

So the takeover is CEFDK's own `script` autorun, the analogue of the CA-1's
U-Boot `boot.scr`, done CEFDK's way. Stored once from the shell with
`script on`; undone with `script off`.

### The autoscript that works

```
emmc rd <gap> 0x6000000 <size>
cache flush
ord4 0xc90a4 = 0x6000000        # linuxKernelBase
ord4 0x837560 = 0x0             # rd_flag=0 (root on p1)
bootlinux "console=ttyS0,115200 pci=realloc,nocrs root=/dev/mmcblk0p1 rootwait rw"
```

**Proven standalone on hardware.** The EA3 self-boots our unsigned 7.1.8 kernel
from eMMC at every power-on, no button, no host, no network, rooted on p1 as a
read-write ext4 filesystem.

### Two non-obvious fixes it needed

**`cache flush` after `emmc rd`, before `bootlinux`.** The shell's `emmc rd` DMAs
the kernel into RAM but does not invalidate the CPU cache, so `bootLinux` read
stale bytes and died with `Invalid or missing kernel`. The shell has a `cache`
command; `cache flush` reconciles it.

**Install to the gap with CEFDK's `emmc wr`, not Linux `dd`.** Linux
`/dev/mmcblk0` and CEFDK's `emmc` command don't agree on the raw gap's byte
offset. The container at `0x400` coincidentally matches; `0x800000` does not. The
install path is `ip set` → `tftp get <server> 0x6000000 bzImage` (CPU-coherent)
→ `cache flush` → `emmc wr <gap> 0x6000000 <size>`. Partition p1 is a real
partition, so Linux `dd` is fine for it, only the raw gap needs `emmc wr`.

## The layout the takeover establishes

All reversible via the restore button, and **p2 is never written**:

| What | Where |
|---|---|
| the signed **stock** kernel | stays in the container at `0x400` — a `script off` software recovery |
| **our** kernel (no initramfs, root=p1) | the ~25 MB raw gap between the container (~7 MB) and p1 (32 MB), at `0x800000` |
| **our** ext4 rootfs | `mmcblk0p1` |
| the autoscript | an MFH item in SPI-NOR |

## What that first real boot proved

openHC 7.1.8 comes up to a full SSH userspace. `e1000` links via the fake-PHY
patch. And the eMMC needed **nothing at all**, with `CONFIG_MMC_SDHCI_PCI` and
`CONFIG_EXT4_FS` added, mainline `sdhci-pci` binds `8086:070b` by class, the
device enumerates `mmcblk0 p1 p2 p3`, and it is read/write from our kernel with
no out-of-tree driver.

## The module posture, if you ever want to go the other way

Read from `/proc/config.gz` on the stock 3.12.74 image, and byte-identical on
3.12.17, so this is a property of the platform rather than one build:

```
CONFIG_MODULES=y                       unsigned modules load...
# CONFIG_MODULE_SIG is not set         ...with no signature check
# CONFIG_MODVERSIONS is not set        vermagic is the only gate
# CONFIG_MODULE_FORCE_LOAD is not set  and it cannot be bypassed
CONFIG_KALLSYMS=y                      non-exported core symbols are resolvable
# CONFIG_DEBUG_SET_MODULE_RONX is not set   module pages stay RWX
# CONFIG_STRICT_DEVMEM is not set      /dev/mem reaches all RAM
# CONFIG_KEXEC is not set              the one thing missing
CONFIG_PHYSICAL_START=0x1000000        vendor kernel loads at 16 MB
```

`sys_kexec_load` is present only as the weak stub, so there's no kexec syscall —
but unsigned modules load freely, which means the gap is fillable from a module
rather than being a wall.

The load addresses matter more than they look: `bootlinux` places a kernel at
**1 MB** and the running vendor kernel sits at **16 MB**, so a replacement
kernel's destination doesn't overlap the kernel doing the copying. That removes
the hardest part of a real kexec.

Since `MODVERSIONS` is off, an out-of-tree module's `vermagic` string is the only
compatibility gate, which is why factory-restoring a unit from a 2018 build to
`3.12.74` mattered: it aligns the running kernel with the last GPL drop that
covers these boards.
