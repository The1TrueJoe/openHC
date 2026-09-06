---
title: Control4's GPL drop
description: What is in the published source, what compiles, what is redistributable, and the correction that cost weeks.
sidebar:
  order: 3
---

**Question:** can we build and distribute our own kernel image for these boards?

**Answer, from inspecting the archives:** the kernel side is redistributable but
does not compile as shipped; the bootloader side is proprietary on every file; and
graphics and media aren't in there at all.

## Where to get it

Two schemes, both live.

**The old 2.x scheme**, still on S3 + CloudFront:

```
http://update.control4.com/open_src/<VERSION>-res/src/<name>+patches.tar.gz
```

Directory listing is disabled (403), but the per-version **XML index** lists every
file with its size and MD5 — `src-<VERSION>-res.xml`. The DM355 unit runs OS 2.9.1
and that directory 403s, but **2.9.0.525559-res** and **2.10.0.540110-res** both
return 200 and contain identical DM355 sources.

Each `+patches.tar.gz` is a pristine upstream tarball plus a `patches/` directory
of Control4's modifications, which is a better disclosure than most. Key packages
for the IO Extender: `linux-2.6.28.10+patches.tar.gz` (50 MB),
`linux-davinci-2.6.28-rc8+patches.tar.gz` (58 MB — has `board-hammer.c` and the
IO driver source as patches), and `u-boot-1.2.0.tgz+patches.tar.gz` (8.7 MB).

**The newer portal**, unauthenticated: <https://open-source.control4.com/>.
`OS-3.3.1.zip` is 2,593,571,045 bytes across 720 entries and is the **last drop
covering the EA-1** — OS-4.x moved to i.MX and dropped both tarballs. The relevant
members:

```
OS 3.3.1/linux-3.12.74+patches.tar.gz   116,982,724 B
OS 3.3.1/cefdk-36+patches.tar.gz        101,931,074 B
OS 3.3.1/tarball_licenses.txt             5,121,373 B
```

## Kernel: real board support, but a missing header breaks the build

Vanilla `linux-3.12.74` plus 54 Intel patches (`-p0`) plus 25 Control4 patches
(`-p1`). The series applies with **zero rejected hunks** — a clean, reproducible
series.

**Genuinely present as GPL source**, and better than a typical drop:

- Intel CE5300/Gen3 platform: `_ce5300_iosf.c`, `ce5300-gpio.c`,
  `intel_media_proc_gen3.c`, `ce_mailbox.c`, `punit_access.c`, `intelce_wdt.c`,
  `arch/x86/hw_mutex/`, `ce5xx_spi_flash.c`
- Ninjago audio: `sound/soc/ninjago/*`, `sound/pci/ninjago/ninjago_fpga.c`, the
  ADAU1451 codecs
- `0042-support-ea1-poe.patch` — EA-1 specific

**Absent, and it's a hard compile failure:**

| Missing | Referenced by |
|---|---|
| `drivers/platform/x86/ninjago_platform/` incl. **`ninjago_platform.h`** | patch `0006` |
| `drivers/control4/` | patch `0039` adds an unconditional `obj-y += control4/` |
| `drivers/char/c4_obj.o` | patch `0004`, unconditional |
| `drivers/char/c4audiosense.c` | patch `0014` |

Four shipped files `#include <ninjago_platform.h>` and call `IS_NYA_ID1()` /
`IS_TR1()` / `IS_DEVBOARD()`: `e1000_main.c`, `e1000_hw.c`, `ath9k/hw.c` and
`ninjago-fpga-dsp.c`. **The EA-1's own Ethernet driver will not compile.** That
header is not four lines of glue you can stub out blind.

There's no EA-1 defconfig either — only Intel's generic `gen3_defconfig.ht`,
which enables no Control4 options. A `/proc/config.gz` pulled off a live unit is
the practical source for that.

**No blobs in the kernel drop** — all 79 patches are text; zero binary files.

The same partial-compliance pattern appears on the DM355 side: the drop includes
`board-hammer.c`, the headers and the U-Boot FPGA loader, but **omits the
`c4fpga.c` / `c4gpio.c` / `c4irout.c` driver bodies**, even though the makefile
patch builds them. Not a blocker, the digital IO needs no driver at all, and the
FPGA loader protocol and register windows are documented in the board file that
*was* published.

## Graphics and media: not in the archive at all

No `pvrsrvkm`, no `gdl`/`gdl_server`, no `pd_hdmi`, no `ismd*`, no
`intel_pic_uart`, no `c4board`, neither source nor binaries, anywhere in the
2.59 GB. No PowerVR userspace, no SGX/SMD firmware.

So a self-built kernel gets you a headless machine unless you take those `.ko`
files and their userspace off your own device, and they are built against the
exact ABI of Control4's kernel config.

**This is no longer the dead end it looks like.** The GPU turned out to be
reachable from a completely different direction: the same SGX545 shipped in Intel
Cedarview, and Intel published the GPL kernel source for that. See
[graphics](/ea/graphics/).

## CEFDK: proprietary, and it verifies kernel signatures

`r36-cefdk-20141030` — 255 MB, but about 245 MB of that's a prebuilt
`i686-cm-linux` GCC 4.5.1 cross toolchain. Real firmware source is roughly 7 MB.
`make gen5` targets CE5300, and the Control4 customisations are all there as
source, including the netboot cookie:

```c
const char* C4_MFG_COOKIE = "C4_COOKIE";     // 0007-control4-bootflow.patch
#define C4_BUTTON_RECOVERY 31
board_ea1p = 5,  // EA1 with POE            // 0032-ea1-poe-board-type.patch
```

:::caution[Licensing is the blocker]
**1,013 of 1,014 CEFDK source files carry the "Intel(R) CEFDK Software License
Agreement" — 0 GPL, 0 BSD.** The agreement text is not in the ZIP, and there is no
written offer or redistribution notice anywhere in the archive.

It also contains binary-only Intel objects with no source: the **DDR memory
reference code** (`meminit.o`, `mrc.o`, `prememinit.o`) and `libproc_*.a`, the one
part you couldn't rebuild even with rights.
:::

### Secure boot — the real gate on custom kernels

CEFDK implements a two-stage chain keyed to Control4: `s2pubkey-c4.h` carries a
Control4 RSA-2048 modulus for stage1→stage2, and
`0010-enable-kernel-authentication.patch` sets `CFG_KERNEL_AUTHENTICATION=1` and
swaps in Control4's key for stage2→kernel.

Enforcement is hardware-gated, from `brd_gen5/sec_boot_linux.c`:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x14) & (2<<21)) ||  // SEC_BOOT_FUSE
    cp_strap_sts_0().strap.sec_boot)                        // SEC_BOOT_STRAP
```

On failure it prints "This is not a valid kernel image with correct signature" and
hangs. **No private keys are in the drop**, and the signing tools ship as binaries
without source.

### …and we read the fuse, at the wrong address

The archive cannot answer whether the fuse is blown, but the register is readable
from a running Linux, the stock kernel has `CONFIG_STRICT_DEVMEM` unset.
Reproducing CEFDK's own check gave:

```
DFX device      : 0000:01:0b.7      (AV_BUS:0b.7, per CEFDK's pciR32)
BAR0            : 0xdf8f0000
reg[BAR0+0x14]  : 0x00000000
SEC_BOOT_FUSE   : 000 (bits 23:21)
secure-boot fuse: CLEAR
```

Two caveats were flagged honestly at the time: the register reads entirely zero,
and the `SEC_BOOT_STRAP` half of the condition lives in a strap-status register we
hadn't located from Linux. Neither was the actual problem.

:::danger[Correction: this reads the WRONG register, and the conclusion does not generalise past the EA1]
The measurement is at `dfx_mbar + 0x14`, which is the **pre-patch Intel**
location. Control4's `cefdk` patch `0022-s3-boot-verify-fix` **moved** the check
to **`dfx_mbar + 0x60` bit 0** and dropped the `rev_id == 4` escape hatch:

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x60) & BIT(0)) ||  /* SEC_BOOT_FUSE=1 */
    cp_strap_sts_0().strap.sec_boot)
    return true;                                            /* enforced */
```

So `+0x14 == 0` says nothing about the shipping check. On an **EA3 (board v2)**
the fuse is **BLOWN**: writing an openHC kernel to the eMMC container and doing a
normal boot gives `VERIFY_S3(kernel bzImage): FAIL` and a watchdog boot loop. The
EA1 (board v1) still boots unsigned kernels, so the fuse likely differs by **board
revision** — an EA1-v1-versus-EA3-v2 change, not an EA-family constant.

**This doesn't block a takeover.** Only CEFDK's *normal* boot verifies. The shell
`bootlinux` command doesn't, and the `script` autorun runs before the verifying
path, so an unsigned kernel boots and *self-boots* persistently regardless of the
fuse. See [secure boot and the takeover](/ea/secure-boot/).

The lesson worth keeping: **we validated against the upstream source when the
shipping binary was patched**, and the patch series was in the same archive we had
already downloaded.
:::

## Verdict

| | Can we build it? | Can we redistribute it? |
|---|---|---|
| Kernel + Control4 kernel drivers | **not as shipped** — missing `ninjago_platform.h`, `drivers/control4/`, `c4_obj`, `c4audiosense` | **yes**, declared GPLv2 |
| CEFDK bootloader | yes (`make gen5`) | **no** — Intel proprietary on every file, binary-only memory reference code, no included agreement |
| Graphics / media / GPU | not present | n/a — but see [graphics](/ea/graphics/), where Intel published the equivalent under GPL for Cedarview |

The practical consequence is the one openHC is built around: **build from
upstream sources with our own patches**, and never redistribute material the
archive gives us no rights to. Installing on a device the user already owns keeps
their own vendor files in place and on their own hardware.

*Reporting what the archives declare; not legal advice.*
