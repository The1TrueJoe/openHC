# Control4's GPL drop — what's actually in it

Question: can we build and distribute our own kernel image for the EA-1?
Answer, from inspecting the archive: **the kernel side is redistributable but does
not compile as shipped; the bootloader side is proprietary; graphics/media are not
in there at all.**

Source: <https://open-source.control4.com/> — unauthenticated. `OS-3.3.1.zip`
(2,593,571,045 B, 720 entries) is the **last drop covering the EA-1**; OS-4.x moved
to i.MX and dropped both tarballs. Relevant members:

```
OS 3.3.1/linux-3.12.74+patches.tar.gz   116,982,724 B
OS 3.3.1/cefdk-36+patches.tar.gz        101,931,074 B
OS 3.3.1/tarball_licenses.txt             5,121,373 B
```

## Kernel: real board support, but a missing header breaks the build

Vanilla `linux-3.12.74` + 54 Intel patches (`-p0`) + 25 Control4 patches (`-p1`).
The series applies with **zero rejected hunks** — a clean, reproducible series.

**Genuinely present as GPL source** — better than a typical GPL drop:
- Intel CE5300/Gen3 platform: `_ce5300_iosf.c`, `ce5300-gpio.c`,
  `intel_media_proc_gen3.c`, `ce_mailbox.c`, `punit_access.c`, `intelce_wdt.c`,
  `arch/x86/hw_mutex/`, `ce5xx_spi_flash.c`
- Ninjago audio: `sound/soc/ninjago/*`, `sound/pci/ninjago/ninjago_fpga.c`,
  ADAU1451 codecs
- `0042-support-ea1-poe.patch` — EA-1 specific

**Absent, and it is a hard compile failure:**

| Missing | Referenced by |
|---|---|
| `drivers/platform/x86/ninjago_platform/` incl. **`ninjago_platform.h`** | patch `0006` |
| `drivers/control4/` | patch `0039` adds unconditional `obj-y += control4/` |
| `drivers/char/c4_obj.o` | patch `0004`, unconditional |
| `drivers/char/c4audiosense.c` | patch `0014` |

Four shipped files `#include <ninjago_platform.h>` and call `IS_NYA_ID1()` /
`IS_TR1()` / `IS_DEVBOARD()`: `e1000_main.c`, `e1000_hw.c`, `ath9k/hw.c`,
`ninjago-fpga-dsp.c`. **The EA-1's own Ethernet driver will not compile.** That
header is not four lines of glue you can stub out blind.

No EA-1 defconfig either — only Intel's generic `gen3_defconfig.ht`, which enables
no Control4 options. Our `backups/ea1/config-3.12.74.gz` (pulled from the live
`/proc/config.gz`) is the practical source for that.

**No blobs in the kernel drop** — all 79 patches are text; zero binary files.

## Graphics and media: not in the archive at all

No `pvrsrvkm`, no `gdl`/`gdl_server`, no `pd_hdmi`, no `ismd*`, no `intel_pic_uart`,
no `c4board` — neither source nor binaries, anywhere in the 2.59 GB. No PowerVR
userspace (`libEGL`, `libGLESv2`, `libsrv_um`, `libIMGegl`), no SGX/SMD firmware.

So a self-built kernel gets you a headless machine unless you take those `.ko` files
and their userspace off your own device — and they are built against the exact ABI
of Control4's kernel config, so a rebuild has to match it closely. This matches the
independent SGX545 research (see `openness.md`).

## CEFDK: proprietary, and it verifies kernel signatures

`r36-cefdk-20141030` — 255 MB, but ~245 MB of that is a prebuilt `i686-cm-linux`
GCC 4.5.1 cross toolchain. Real firmware source is ~7 MB. `make gen5` targets
CE5300/Berryville, and the Control4 customizations are all there as source,
including our netboot cookie:

```c
const char* C4_MFG_COOKIE = "C4_COOKIE";     // 0007-control4-bootflow.patch
#define C4_BUTTON_RECOVERY 31
board_ea1p = 5,  // EA1 with POE            // 0032-ea1-poe-board-type.patch
```

**Licensing is the blocker: 1,013 of 1,014 CEFDK source files carry "Intel(R) CEFDK
Software License Agreement" — 0 GPL, 0 BSD.** The agreement text is not in the ZIP,
and there is no written offer or redistribution notice anywhere in the archive.
It also contains binary-only Intel objects with no source: the **DDR memory
reference code** (`meminit.o`, `mrc.o`, `prememinit.o`) and `libproc_*.a` — the one
part you could not rebuild even with rights.

### Secure boot — the real gate on custom kernels

CEFDK implements a two-stage chain keyed to Control4:
- `s2pubkey-c4.h` — Control4 RSA-2048 public modulus for stage1→stage2
- `0010-enable-kernel-authentication.patch` sets `CFG_KERNEL_AUTHENTICATION=1` and
  swaps in Control4's key for stage2→kernel

Enforcement is **hardware-gated** (`brd_gen5/sec_boot_linux.c:61`):

```c
if ((*(volatile uint32_t *)(dfx_mbar + 0x14) & (2<<21)) ||  // SEC_BOOT_FUSE
    cp_strap_sts_0().strap.sec_boot)                        // SEC_BOOT_STRAP
```

On failure it prints "This is not a valid kernel image with correct signature" and
hangs. **No private keys are in the drop**, and the signing tools ship as binaries
without source.

### …and we read the fuse: it is CLEAR (measured, no serial needed)

The archive can't answer whether the fuse is blown — but the register is readable
from a running Linux. Reproducing CEFDK's own check against `/dev/mem` (the stock
kernel has `CONFIG_STRICT_DEVMEM` **unset**, so this is permitted) gives:

```
DFX device      : 0000:01:0b.7      (AV_BUS:0b.7, per CEFDK's pciR32)
BAR0            : 0xdf8f0000
reg[BAR0+0x14]  : 0x00000000
SEC_BOOT_FUSE   : 000 (bits 23:21)
PCI 0:0.0 rev   : 0x0c
secure-boot fuse: CLEAR
```

**Signature enforcement is off on this unit — a self-built, unsigned kernel should
boot.** Two caveats kept honest: the register reads `0x00000000` entirely, and the
`SEC_BOOT_STRAP` half of the condition lives in a strap-status register we have not
located from Linux, so a board strap could in principle still assert it. The only
way to be certain is to actually boot one (netboot via mfg mode writes nothing —
see `recovery.md`).

The check is CEFDK's, from `brd_gen5/sec_boot_linux.c`:

```c
dfx_mbar = BAR0 of PCI <AV_BUS>:0b.7
if ((*(uint32_t*)(dfx_mbar + 0x14) & (2 << 21))   /* SEC_BOOT_FUSE, bits 23:21 */
    || cp_strap_sts_0().strap.sec_boot)           /* SEC_BOOT_STRAP */
    if (4 != pciread8(0, 0, 0, 0x8))              /* ...unless rev id == 4 */
        return true;                              /* signature enforced */
```

This was a one-shot measurement, not something the firmware ships a tool for —
the answer does not change at runtime, and a `/dev/mem` poker is not worth
carrying on every box for a constant.

## Verdict

| | Can we build it? | Can we redistribute it? |
|---|---|---|
| Kernel + Control4 kernel drivers | **not as shipped** — missing `ninjago_platform.h`, `drivers/control4/`, `c4_obj`, `c4audiosense` | **yes**, declared GPLv2 |
| CEFDK bootloader | yes (`make gen5`) | **no** — Intel proprietary on every file, binary-only MRC, no included agreement |
| Graphics / media / GPU | not present | n/a — must come off the user's own device |

Practical consequence for openHomeController: this **reinforces the userspace-overlay
strategy**. Shipping our userspace and installing it on a device the user already
owns stays clean. Shipping a full flashable image would require either replacing the
missing Control4 kernel glue ourselves *and* excluding the vendor graphics stack, or
redistributing material the archive gives us no rights to.

*Reporting what the archive declares; not legal advice.*
