---
title: The impossible GPU
topic: Graphics
summary: Nobody has ever written a free driver for PowerVR Series5. I didn't have to — the same GPU shipped in a chipset Intel published GPL kernel source for.
description: Getting GLES2 out of the PowerVR SGX545 on a mainline kernel.
sidebar:
  order: 9
  label: The impossible GPU
---

The EA family has a PowerVR SGX545 inside its CE5310. The received wisdom about
PowerVR Series5 is unambiguous and, in general, correct. Nobody has ever produced
a working free driver, several serious attempts have died, and the mainline
`powervr` driver that landed in 6.8 is Rogue-only, Series6 and later. It doesn't
cover SGX.

So a mainline kernel on these boards loses video. That was the settled position
for a long time, and it's why openHC is headless on every board.

It's also wrong for this specific part, for one specific reason.

## SGX545 also shipped in Intel Cedarview

The same GPU core went into Intel's Cedarview chipsets as the GMA 3600/3650. And
for Cedarview, Intel published the complete GPL `services4`/`srvkm` kernel source.
It's still sitting in the Ubuntu archive:

```
old-releases.ubuntu.com/ubuntu/pool/multiverse/c/cedarview-drm-drivers/
```

The `orig` tarball ships the directory skeleton empty; the actual source is in a
3.47 MB quilt patch. Apply the series and you get 51 `.c` files and 93 headers,
78,349 lines, GPL v2, "Copyright (C) Imagination Technologies Ltd.", in the exact
directory layout named in the build paths embedded in our own stock
`pvrsrvkm.ko`.

Including `hwdefs/sgx545defs.h`, 62 KB of SGX545 register definitions. The
programmer's reference everyone assumed had never been published. Intel published
it, machine-readable, under GPL, in 2012.

There's a matched i386 userspace package alongside it at the same DDK version,
carrying `libEGL`, `libGLESv2`, `libglslcompiler` and `libusc`. The USSE shader
compiler, the piece nobody has ever reimplemented, is a shipped binary.

So the FOSS-Mesa route — genuinely multiple person-years, and it has defeated
every group that has tried since 2011 — is simply not the route. The folklore is
true and irrelevant.

## Three checks before believing it

**The core revision matches exactly.** `sgxerrata.h` enumerates SGX545 revisions,
and our EA3 reports `1014`. Cedarview builds for `10131`. Those two carry the
identical erratum set: one workaround, `FIX_HW_BRN_SAMPLE_CACHE`, and nothing
else. Rebuilding for our silicon changes one `-D`.

**The trees are the same tree.** Our stock `pvrsrvkm.ko` isn't stripped and embeds
source paths. It was compiled from 43 files, and 40 of the 43 are present in
Intel's tree. The three missing ones are a debug refcount wrapper, a mutex
wrapper, and Android explicit fencing the 1.7 userspace doesn't use.

**The build path is self-contained.** Of 17 locally-included headers absent from
the tree, all 17 are unreachable in an SGX545 build: other cores behind
`#if defined(SGXnnn)`, video-decode bridges behind their own options, and
kernel-supplied headers.

Control4's driver is a stock Imagination DDK tree with an Intel CE system-config
layer, exactly as I'd guessed, and Intel published the equivalent tree five DDK
minor versions earlier with the SGX545 definitions intact.

## The firmware question, which turned out not to be one

The expected wall was the microkernel. PowerVR parts run an EDM — Event Driven
Microkernel — on the GPU, and a driver that has to contain, sign or synthesize
firmware is a much harder problem.

Measured on our own `libsrv_init.so`: the microkernel is in userspace, and it
isn't signed. A 9.8 KB `.text` with one exported symbol, and a 54 KB `.rodata`
that opens with the microkernel's config-key table (`MaxEDMTasks`,
`DisableClockGating`, `USETmpRegCount`, `YUVCoef00`…) and ends in 8-byte-aligned
USSE machine words. Cedarview's equivalent is the same size class, and 34% of its
32-byte windows appear verbatim in ours. Same microkernel, two builds.

So a kernel driver never has to hold firmware. It only has to accept the upload.
The gate that would have killed this wasn't a gate.

## The correction: DRM is not optional

An earlier draft of this work assumed I could build with `SUPPORT_DRI_DRM` off,
use the DDK's plain character-device path, and drop the vendored DRM tree
entirely. That would have made the port trivially small.

It was wrong, and three things say so. Both candidate userspace blob sets link
`libdrm.so.2` and import `drmOpen`/`drmClose`/`drmCommandWrite`. Control4's own
module contains `pvr_drm.c` and `sPVRDrmIoctls` in its embedded strings. And
`/dev/dri/card0` on the stock system *is* that node.

Userspace reaches the driver through DRM, so a DRM device has to exist.

What stayed true is that Cedarview's vendored Linux 3.2 DRM subsystem isn't
needed. The DDK's actual DRM usage is tiny — 479 lines, using DRM as a named
device node plus five vendor ioctls, so it got rewritten against mainline DRM.
The port is 43 files plus one shim, not 558.

The 3.2 → 7.1 API surface was small and enumerable, as expected: removed macros,
signature churn in `get_user_pages` and `access_ok`, renames, and four structural
items — procfs, timers, five-level page tables, and the page-table walk.

## It works

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

Blue background, green triangle, on the panel.

Four bugs stood between "the module builds" and that, and none was findable
without hardware:

| # | Bug | Symptom |
|---|---|---|
| 1 | fops missing `FOP_UNSIGNED_OFFSET` | userspace could never `open()` the DRM node |
| 2 | bus id hardcoded to Cedarview's `0000:00:02.0` **inside the blob** | `drmOpen` returned −1, no ioctl issued, empty dmesg |
| 3 | the DDK's core-rev exception loop counts pairs but indexes elements | our exception was never compared |
| 4 | fbdev PFN check assumes `remap_pfn_range` semantics | blocked every GLES surface |

Bug 1 is the instructive one. Since 6.12, `drm_open_helper()` requires the file
operations to declare `FOP_UNSIGNED_OFFSET`, because DRM passes the mmap offset
through as an unsigned token rather than a signed file position, and `PVRMMap`
keys its lookup on exactly that. `DRM_GEM_FOPS` sets the flag for GEM drivers;
ours is hand-rolled because the driver owns no GEM objects. The module builds,
loads, binds and registers `card0` perfectly well without it. Only `open()` fails.

Bug 2 is the one to remember when reusing vendor blobs. `libsrv_um` calls
`drmOpen(NULL, "pci:0000:00:02.0")`, Cedarview's address, not ours. Because the
*name* argument is NULL, libdrm has no fallback to reach. The open just fails, no
ioctl is issued, dmesg is empty. Without the shim a GLES program isn't degraded,
it's broken. It's installed via `/etc/ld.so.preload` rather than per caller,
because a browser engine does its rendering in a separate process and that's the
process that needs it.

There was also an insight no amount of reading would have produced. `SGX_CORE_REV`
has to follow the microkernel, not the silicon, because it selects the shared
interface struct layout.

## What it costs, and what it's worth

1.25 MB of the boot window, taking an image from 64.6% to 80.1% of CEFDK's copy
ceiling. Almost none of that is the driver: the build compiles 41 objects for the
driver and 417 for DRM core — atomic modeset, DP/MST, HDCP, DSC, EDID, bridge and
panel helpers — none of which the driver calls.

It isn't trimmable from a defconfig. `CONFIG_DRM=y` defaults `DRM_BRIDGE` and
`DRM_PANEL` on, and those select helper libraries with no prompt. A fragment
trying to unset them is silently ignored. I tried it; the image came back
byte-identical. Reclaiming the space needs a kernel patch.

As for performance, the benchmark is a pleasant surprise and a diagnosis at the
same time. At 720x480:

| layers | opaque | blended | textured |
|---|---|---|---|
| 1 | 30.3 fps | 28.8 | 28.8 |
| 2 | 28.8 | 28.8 | 28.8 |
| 8 | 28.8 | 28.8 | 28.8 |

Flat. Eight layers of full-screen blended compositing cost exactly what one does,
so fill rate is nowhere near the limit. The GPU is massively over-provisioned for
a launcher UI. The ~29 fps ceiling is a fixed per-frame cost, which points squarely
at the framebuffer backend blitting the whole surface on every swap.
`eglSwapInterval(0)` is accepted and changes nothing, so it isn't vsync. If 60 fps
is ever wanted, attack the present path, not the rendering.

## The ceiling that has nothing to do with the GPU

The intended use is a browser-based launcher, and there the blocker is memory.
Measured on the live EA3: 186 MB.

That's very likely below what a modern engine needs, since it's multi-process with
a UI process and a web process each carrying its own heap.

Except the memory is there. CEFDK's e820 map gives Linux the first 200 MB and
reserves 1.8 GB for the Intel CE media stack that openHC never loads. And it's
genuinely idle DRAM rather than address space, verified by writing test patterns
at 1 GB and reading them back, checking for aliasing into low memory, and finding
much of the range still holding CEFDK's own boot-time address-walking pattern.

A `memmap=` argument reclaims it. It's written out, commented, and deliberately
not enabled, because if any of that range is live after all the kernel doesn't
warn — it corrupts and panics, and this board's only recovery is the
factory-restore button and the bootloader shell. It should be switched on with a
serial console attached, and widened in stages.

Same principle as everywhere else here: try the cheap thing first. An
out-of-memory kill costs a reboot. A bad `memmap=` past the end of real DRAM isn't
recoverable remotely.
