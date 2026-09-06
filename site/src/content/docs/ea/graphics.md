---
title: Graphics — the PowerVR SGX545
description: GLES2 on a Series5 PowerVR, via the GPL kernel source Intel published for Cedarview.
sidebar:
  order: 10
---

**Verdict: it works.** GLES2 rendering is confirmed on hardware. The kernel driver
builds clean against 7.1.8, binds the device, registers `/dev/dri/card0`, and the
vendor userspace compiles and links shaders through it.

The prevailing "Series5 SGX is a dead end" assumption is **wrong for this part**,
for a specific reason: SGX545 also shipped in Intel **Cedarview** (GMA 3600/3650),
and **Intel published the complete GPL `services4`/`srvkm` kernel source for it.**
It is still downloadable from the Ubuntu archive. We don't need to reverse
engineer a kernel driver, and we do not need a shader compiler.

The folklore is true in general, nobody has ever produced a Mesa driver for
Series5, and irrelevant here, because the FOSS-Mesa path is not the path.

## The decisive find

```
old-releases.ubuntu.com/ubuntu/pool/multiverse/c/cedarview-drm-drivers/
  cedarview-drm-drivers_20120717.orig.tar.gz
  cedarview-drm-drivers_20120717-0ubuntu1.debian.tar.gz
```

The `orig` tarball ships the directory skeleton **empty**; the actual source is in
`debian/patches/07_kernel-3.2-cdv.diff`, 3.47 MB. Apply the quilt series and you
get:

| | |
|---|---|
| Tree | `staging/cdv/pvr/` |
| DDK version | **1.7.862890** |
| Size | 51 `.c` + 93 `.h`, **78,349 lines** |
| License | **GPL v2**, Imagination Technologies, `gpl-support@imgtec.com` |
| Layout | `services4/srvkm/{common,devices/sgx,env/linux,bridged/sgx,hwdefs,include}` — the exact layout named in `pvrsrvkm.ko`'s embedded build paths |

Including **`hwdefs/sgx545defs.h` — 62 KB of SGX545 register definitions.** The
programmer's reference everyone assumed had never been published. Intel published
it, machine-readable, under GPL, in 2012.

There is a **matched i386 userspace** package at the same DDK version, carrying
`libEGL`, `libGLESv2`, `libIMGegl`, `libsrv_um`, `libglslcompiler`, `libusc` and
five window-system backends. One of them is
**`libpvrPVR2D_LINUXFBWSEGL.so` — EGL directly on `/dev/fb0`, no X server**, which
our stock blob set does not have and which is a *better* fit for openHC than the
stock one.

## Three checks before believing it

**The core revision matches exactly.** `sgxerrata.h` enumerates SGX545 revisions
`100, 109, 1012, 1013, 10131, 1014, 10141`; our EA3 reports **1014**. Cedarview
builds for `10131`. Those two carry the **identical** erratum set —
`FIX_HW_BRN_SAMPLE_CACHE` and nothing else, so rebuilding for our silicon changes
one `-D` and produces the same workaround set as a shipped, known-working driver.

**The trees are the same tree.** Our stock `pvrsrvkm.ko` isn't stripped and embeds
`__FILE__` paths: it was compiled from 43 files, and **40 of the 43 are present**
in Intel's tree. The missing three are a debug refcount wrapper (no-op in release
builds), a mutex wrapper, and Android explicit fencing that the 1.7 userspace
doesn't use.

**The build path is self-contained.** Of 17 locally-included headers absent from
the tree, all 17 are unreachable in an SGX545 build, other cores behind
`#if defined(SGXnnn)`, video-decode bridges behind their own options, and
kernel-supplied headers.

## The microkernel is in userspace, and it is not signed

The expected wall was firmware. Measured on our own `libsrv_init.so`:

```
.text    0x2648   (9,800 bytes)   — one exported symbol: SrvInit
.rodata  0xd2f8  (54,008 bytes)   — entropy 4.53 bits/byte
```

The `.rodata` opens with the microkernel's config-key table — `MaxEDMTasks`,
`DisableClockGating`, `USETmpRegCount`, `CacheCtrl`, `YUVCoef00`…`YUVCoef14` — and
the tail is 8-byte-aligned USSE machine words. "EDM" is Imagination's Event Driven
Microkernel.

Cedarview's equivalent has a 53,640-byte `.rodata`, same size class, and **34 %
of its 32-byte windows appear verbatim in ours.** Same microkernel, two DDK builds.

**A kernel driver never has to contain, sign or synthesize firmware.** It only has
to accept the upload. The hard gate wasn't a gate.

The remaining ABI constraints are real but ordinary, and all four are enforced by
string-identified checks in the module — DDK version exact-match, struct sizes,
a build-option bitmask, and a core-rev check with a skip path. All are satisfied
by construction if you build the kernel module from the same DDK tree and flag set
as the userspace you pair it with. The build-option bitmask was checked ahead of
time and is identical to Cedarview's release build.

## The correction: DRM is not optional

An earlier draft assumed we could build with `SUPPORT_DRI_DRM` off, use the DDK's
plain character-device path, and drop the vendored DRM tree entirely.

**That's wrong.** Three things say so: both candidate userspace blob sets link
`libdrm.so.2` and import `drmOpen`/`drmClose`/`drmCommandWrite`; Control4's own
`pvrsrvkm.ko` contains `pvr_drm.c` and `sPVRDrmIoctls` in its embedded strings;
and `/dev/dri/card0` on the stock system is that node.

Userspace reaches the driver *through DRM*, so a DRM device has to exist.

What is still true is that Cedarview's vendored Linux 3.2 DRM subsystem isn't
needed. The DDK's DRM usage is tiny — `pvr_drm.c` is 479 lines and uses DRM purely
as a named device node plus five vendor ioctls, so it was rewritten against
mainline DRM. **The port is ~43 files plus one shim, not 558.**

The 3.2 → 7.1 API surface was small and enumerable: removed macros (`__devinit`,
`asm/system.h`, `drmP.h`), signature churn (`get_user_pages`, `access_ok`,
`class_create`, `__vmalloc`), renames (`mmap_sem`, `del_timer_sync`,
`VM_RESERVED`), and four structural items — procfs, timers, five-level page
tables, and the page-table walk.

`CONFIG_MODULES=n` in openHC is **not** a blocker here, because we build from
source and it goes in-tree as `obj-y`. It *is* an absolute blocker for the stock
`.ko`, which additionally has `vermagic=3.12.74` and depends on Intel's OSAL
shims.

## The port

Implemented in a separate repository, `sgx545-ce`, structured so the
upstream/ours boundary stays verifiable — the first commit is Intel's tree
imported verbatim, everything after it's ours.

```
pvrsrvkm.ko   156 KB   Linux 7.1.8 / i686   0 unresolved symbols
alias:   pci:v00008086d0000089B
depends: (none)
```

For scale, Control4's stock DDK 1.12 module is 182 KB and depends on two Intel
shim modules.

## Verified on silicon

```
[drm] Initialized pvrsrvkm 1.7.862890 for 0000:01:02.0 on minor 0
```

And the two things that could only ever be settled on hardware came back as
predicted, read straight off BAR0:

| Register | Value | Meaning |
|---|---|---|
| `CORE_REVISION` @0x20 | `0x0001000E` | **1.0.14 → SGX_CORE_REV 1014** — matches the build default exactly |
| `CORE_ID` @0x1C | `0x01150000` | stable across reads → **core is clocked and alive at boot** |
| register window | **BAR0+0** | the CE5300-specific offset, reverse-engineered from Control4's binary, **confirmed** |

## GLES2, on the panel

```
GL_VENDOR   : Imagination Technologies
GL_RENDERER : PowerVR SGX 545
GL_VERSION  : OpenGL ES 2.0 build 1.7@862890

  compile shaders (USSE compiler)       ok
  link program                          ok
  draw a triangle                       ok
  read back the centre pixel            rgba(0,255,0,255) -- the shader's green
  eglSwapBuffers (to the panel)         ok
```

Blue background, green triangle, visually confirmed.

### Four bugs stood between "it builds" and that

None was findable without hardware.

| # | Bug | Symptom |
|---|---|---|
| 1 | fops missing `FOP_UNSIGNED_OFFSET` | userspace could never `open()` the DRM node |
| 2 | bus id hardcoded to Cedarview's `0000:00:02.0` **inside the blob** | `drmOpen` returned −1, no ioctl ever issued, empty dmesg |
| 3 | the DDK's core-rev exception loop bound counts pairs but indexes elements | our exception was never compared |
| 4 | fbdev PFN check assumes `remap_pfn_range` semantics | blocked every GLES surface |

**Bug 1**, in detail, because it's the instructive one. Since 6.12,
`drm_open_helper()` contains:

```c
if (WARN_ON_ONCE(!(filp->f_op->fop_flags & FOP_UNSIGNED_OFFSET)))
        return -EINVAL;
```

DRM passes the mmap offset through as an unsigned token rather than a signed file
position, and `PVRMMap` keys its lookup on exactly that. `DRM_GEM_FOPS` sets the
flag for GEM drivers; ours is hand-rolled because the driver owns no GEM objects,
so it had to be set explicitly. **The module builds, loads, binds and registers
`card0` perfectly well without it.** Only `open()` fails.

**Bug 2** is the one to remember when reusing vendor blobs: `libsrv_um` calls
`drmOpen(NULL, "pci:0000:00:02.0")`. Because the *name* argument is NULL, libdrm
has no name-based fallback to reach — the open just fails, with no ioctl issued
and nothing in dmesg. Without the shim a GLES program is not degraded, it's
broken. It is installed via `/etc/ld.so.preload` rather than per caller, because a
browser engine does its rendering in a separate process and that's the process
that needs it.

Plus the insight that **`SGX_CORE_REV` must follow the microkernel, not the
silicon**, because it selects the shared interface struct layout.

## Performance, and what it diagnoses

At 720x480:

| Layers | Opaque | Blended | Textured |
|---|---|---|---|
| 1 | 30.3 fps | 28.8 | 28.8 |
| 2 | 28.8 | 28.8 | 28.8 |
| 4 | 28.8 | 28.8 | 28.8 |
| 8 | 28.8 | 28.8 | 28.8 |

**Flat.** Eight layers of full-screen blended compositing cost exactly what one
does, so fill rate is nowhere near the limit, the GPU is massively
over-provisioned for a launcher UI. The ~29 fps ceiling is a fixed per-frame cost,
pointing at the LinuxFB backend blitting the whole 720x480x4 surface on every
swap. `eglSwapInterval(0)` is accepted and changes nothing, so it isn't vsync.

If 60 fps is wanted, attack the **present path** — a page-flipping backend ships
alongside the blit one, not the rendering.

For comparison the CPU manages ~302 Mpix/s of memcpy-to-framebuffer, so for a
*static* 2D launcher the CPU isn't hopeless either. The GPU's win is shaders,
transforms and effects coming for free.

## What it costs

**1.25 MB of the boot window**, taking an image from 64.6 % to 80.1 % of CEFDK's
copy ceiling:

| | bytes | % of copy window |
|---|---|---|
| without the `sgx` feature | 5,468,672 | 64.6 % |
| with it | 6,779,392 | **80.1 %** |
| delta | **+1,310,720** | |

Almost none of that's the driver. The build compiles **41 objects for the driver
and 417 for DRM core** — atomic modeset, DP/MST, HDCP, DSC, SCDC, EDID, bridge and
panel helpers — none of which the driver calls.

**It isn't trimmable from a defconfig.** `CONFIG_DRM=y` defaults `DRM_BRIDGE` and
`DRM_PANEL` on, and those select helper libraries with no prompt. A fragment
trying to unset any of them is silently ignored, tried it, the image came back
byte-identical. Reclaiming the space needs a kernel patch making the helper
libraries conditional on a driver actually wanting them.

## The 720x480 ceiling, and why

The panel is 720x480 and that isn't a setting anywhere in openHC. CEFDK leaves
the display pipe enabled with a 720x480 timing scanning ARGB8888 out of
`0x7fc00000`; all `ce5300-fb` does is hand that existing buffer to
`simple-framebuffer`. **simplefb has no modeset path by design**, it describes a
framebuffer somebody else configured.

So the UI is SD until someone drives the display controller properly, which means
a real modeset driver against the register map in the vendor's `gdl_server.ko`.
That is a separate project from 3D and is not required for any of the above.

## The constraint that actually decides the UI: RAM

Measured on the live EA3: **186 MB**. That's very likely below what a modern
browser engine needs — it is multi-process, with a UI process and a web process
each carrying its own heap.

Except the memory is there. CEFDK's e820 map:

```
[mem 0x00100000-0x0c7fffff]  System RAM        <- 200 MB
[mem 0x0c800000-0x7fffffff]  device reserved   <- 1.8 GB
```

The board has 2 GB fitted. Linux gets the first 200 MB; the rest is carved out for
the Intel CE media stack that openHC never loads. The reference design for this
SoC is a set-top box, and a controller running a launcher isn't one.

**Evidence it's real, idle DRAM and not merely address space:**

| Check | Result |
|---|---|
| `/proc/iomem` claims inside the 1.8 GB | exactly one: `ce5300-fb` at `0x7fc00000` |
| does DRAM exist at the top of the range? | yes — simplefb draws through it and the panel shows it |
| write test at `0x40000000` (1 GB) | `0xa5a5a5a5` and `0x5a5a5a5a` both read back; original restored |
| aliasing into low memory? | no — neighbour word undisturbed, low addresses read different values |
| has anything written it since boot? | no — much of it still holds CEFDK's address-walking test pattern |

`memmap=1024M@0x10000000` takes 1 GB starting at 256 MB and stops well short of
the framebuffer, which would put the box at ~1.2 GB.

:::danger[Written out, deliberately not enabled]
If any of the reclaimed range is live after all, **the kernel doesn't warn, it
corrupts and panics**, and this board's only recovery is the factory-restore
button and the CEFDK shell. Switch it on with a serial console attached and widen
in stages. It's also specific to the EA3 v2 measured here; a `memmap=` past the
end of real DRAM isn't recoverable remotely.

Sequencing: run the workload at 186 MB first and see what happens. An
out-of-memory kill costs a reboot and is informative. Reclaiming is the step that
can strand the board.
:::

## Recovered material index

| What | Where | DDK |
|---|---|---|
| **Intel SGX545 kernel source, GPL v2** | `old-releases.ubuntu.com/.../cedarview-drm-drivers/` | **1.7.862890** |
| **Matched i386 userspace blobs** (incl. `libusc.so`, LinuxFB backend) | `old-releases.ubuntu.com/.../cedarview-graphics-drivers/` | **1.7.862890** |
| Port prior art (→ Linux 3.10/3.11) | `github.com/thomas001/cedarview-drm` | 1.7 |
| TI `services4`, modern kernels, **no SGX545 hwdefs** | `git.ti.com/cgit/graphics/omap5-sgx-ddk-linux/` | 1.14, 1.17 |
| SGX540 reverse engineering (partial, dormant) | `codeberg.org/Garnet/sgx540-reversing` | n/a |

The userspace license is Intel's binary-redistribution license: redistribution in
binary form is permitted with notice, and it **forbids reverse engineering the
blobs**. Using them as-is is fine; disassembling them isn't.

### Negative results, verified

- Control4's CEFDK drop contains **no** graphics, bootloader only.
- **No CE4100/CE5300 licensee ever published graphics source.** The platform was
  contractually closed; SDK access required an Intel field engineer. Intel never
  published the CE graphics SDK and it has not leaked. That grievance stands, it
  just no longer matters, because Cedarview is the same GPU and Intel *did*
  publish that one.
- TI's tree has `sgx530defs.h`, `sgx540defs.h` and `sgx544defs.h` and **no
  `sgx545defs.h`** — TI never shipped a 545. It's a porting scaffold, not a
  source.
- **A pure FOSS Mesa/Gallium route isn't viable.** The high-water mark for SGX
  reverse engineering is a partial assembler, explicitly unfinished on instruction
  encoding and semantics, SGX540 only, dormant since 2022. Mainline's `powervr`
  driver is Rogue-only. Even with `sgx545defs.h` in hand, which is the register
  map, not the ISA. This is multiple person-years, because the missing piece is
  the shader compiler.
- **OpenCL is worse, not better.** SGX545 does support OpenCL 1.0 and the DDK has
  a module for it, but **neither Intel nor Control4 ever shipped that `.so`**. It
  needs the same kernel driver *plus* a userspace runtime nobody distributed.
