---
title: Graphics — the PowerVR SGX545
description: GLES2 on a Series5 PowerVR with an open stack: the powervr_sgx kernel driver, its open microkernel and Mesa's sgx driver.
sidebar:
  order: 10
---

**The EA's GPU runs a fully open stack:** [powervr_sgx](https://github.com/The1TrueJoe/powervr_sgx)
0.2.1, a DRM kernel driver, an open microkernel and a Mesa gallium driver for the
SGX545. No Intel or Imagination code runs on the box, and the userspace is
native 64-bit. The on-screen web view is the HC-800's: WebKit's WPE Platform DRM
display, on Mesa.

Until this, the GPU ran Intel's 32-bit Cedarview DDK 1.7 under IA32 emulation,
on a port of its GPL kernel driver. That story is in
[The impossible GPU](/log/the-impossible-gpu/).

## The stack

| Layer | What | Where it comes from |
|---|---|---|
| Kernel driver | `powervr_sgx`, render-only (`/dev/dri/renderD128`) | `drm/powervr_sgx` of the `board/ea/common/powervr_sgx` submodule, mirrored to `drivers/gpu/drm/powervr_sgx` (`board/external.mk`) |
| Microkernel | `/lib/firmware/powervr_sgx/sgx545_ukernel.bin` (MIT) | the release's firmware package (`powervr-sgx-firmware`) |
| Display | `simpledrm` on the framebuffer CEFDK leaves running (`/dev/dri/card0`, and `/dev/fb0` through fbdev emulation) | mainline |
| GL | Mesa 26.2.5 with the `sgx` gallium driver: EGL, OpenGL ES 2.0, GBM | Buildroot's `mesa3d` with the submodule's `mesa/` overlay (`BR2_PACKAGE_MESA3D_GALLIUM_DRIVER_SGX`) |
| Web view | WPE WebKit's DRM platform, run by `webview` | Buildroot's `wpewebkit` (WPE Platform, `build/buildroot-patches/0005`) |

The GPU only renders. Mesa's kmsro pairs it with simpledrm: a buffer to show is
a simpledrm buffer the SGX renders into. simpledrm copies it to the CEFDK
framebuffer, because nothing drives the display controller itself (see
[the 720x480 ceiling](#the-720x480-ceiling-and-why)).

The driver loads the microkernel when it probes and starts the GPU itself, so
nothing runs at boot for the GPU. On the bootstrap kernel `powervr_sgx` is a
module (udev loads it from p1). The real kernel builds it in, and the
microkernel with it (`EXTRA_FIRMWARE`, `build/mk-real-kernel.sh`), because it
probes before p1 is mounted.

dmesg on a good boot:

```
powervr_sgx 0000:01:02.0: SGX core id 0x01150000 revision 0x0001000e: Intel CE5300 ...
powervr_sgx 0000:01:02.0: powervr_sgx/sgx545_ukernel.bin: open microkernel ..., features 0x2
powervr_sgx 0000:01:02.0: microkernel started
```

`kmscube` is in the image as a GL check: it draws a spinning cube on the
display. Stop the web view first, since only one program can drive the
display.

A job that hangs the GPU is reset (`job timed out ...; resetting`, then
`microkernel started again`); the GL context that caused it is reported lost
and other clients carry on.

## What the panel shows

**The splash, by default.** `S01splash` draws the openHC banner on `/dev/fb0` at
boot and `S40net` redraws it with the box's address once the network is up.
Nothing starts a browser: a fresh image runs no webview and opens no listener
for the panel.

The on-panel web view is opt-in. In the web UI, **Display → Show a web page on
the controller's own screen** (`POST /api/display`, kept in
`/opt/ohc/display.conf`) has `S95ohc-display` start `webview` on WPE WebKit's
DRM display, loading the dashboard webd already serves (or any URL you give
it). Turning it off stops the view and redraws the splash, no reboot. The
script and `webview` are the HC-800's too (`packages/webview`).

## The 720x480 ceiling, and why

The panel is 720x480 and that isn't a setting anywhere in openHC. CEFDK leaves
the display pipe enabled with a 720x480 timing scanning ARGB8888 out of
`0x7fc00000`; the board device tree just describes that existing buffer to
`simple-framebuffer` (a `/chosen/framebuffer` node). **simplefb and simpledrm
have no modeset path by design**: they describe a framebuffer somebody else
configured. simpledrm offers it as one fixed mode, and every frame shown is a
CPU copy of its damaged area into that framebuffer.

So the UI is SD until someone drives the display controller properly, which means
a real modeset driver against the register map in the vendor's `gdl_server.ko`.
That is a separate project from 3D and is not required for any of the above.

## Keyboards and mice

The on-panel UI takes USB keyboards and mice, plugged in directly or through a
hub (the `usb-input` feature brings the kernel side: evdev, usbhid, hid-generic).
WebKit's DRM display reads them through libinput, as on the HC-800. The layout is
US unless `XKB_DEFAULT_LAYOUT` says otherwise.

simpledrm has no cursor plane, so the page draws the cursor itself: `webview` injects a
small script that keeps a click-through element under the pointer. It shows a hand
over links and an I-beam over text fields, and hides while you type or after a few
idle seconds. `WEBVIEW_CURSOR=0` turns it off for a touch-only panel. The HC-800
uses the same cursor.

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
| `/proc/iomem` claims inside the 1.8 GB | exactly one: the framebuffer at `0x7fc00000` (then `ce5300-fb`, now simplefb from the board DTB) |
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

## Kexec out of a GPU kernel breaks the next one

Everything in this section was measured with the DDK's `pvrsrvkm`. It hasn't
been measured again with `powervr_sgx`, so the rule below still stands.

Once `pvrsrvkm` had bound the SGX (01:02.0), **a kexec out of that kernel leaves
the next kernel broken**, every time. Measured on an EA1, 2026-10-26:

- **Legacy-block reads run one behind.** Reads of the HPET (`0xfed00000`) and
  IOAPIC (`0xfec00000`) return the *previous* read's value. The HPET looks frozen
  (`clocksource: Watchdog hpet interval: 0ns`), the TSC is marked unstable,
  timers die, and interrupts are misrouted.
- **DMA is dead.** SDHCI times out waiting for its ADMA interrupt. e1000 reports
  `Detected Tx Unit Hang` with TDH stuck at 0. EHCI gets -110.

It doesn't depend on the incoming kernel or initrd. MTRRs and PAT are identical
on both sides, and there is no DMAR table. A second kexec on its own is fine:
bootstrap → kexec'd bootstrap → fat kernel boots clean. What matters is whether
the SGX driver probed in the *outgoing* kernel. With it blacklisted
(`initcall_blacklist=PVRSRVDrmInit`), the same kexec is clean.

**The normal boot is not affected.** The bootstrap kernel that `S00kexec` runs
from never binds the GPU, so its hand-off to the fat kernel is clean.

### GPU idle: fixed by holding the core in reset

After a **cold** boot where nothing rendered, unbinding `pvrsrvkm` and then
writing `EUR_CR_SOFT_RESET` (`0x80`) = `0x3f` holds BIF, DPM, TA, USE, ISP and
TSP in reset. The next kernel then comes up healthy. This was confirmed on two
different fat kernels.

The fat kernel does it by itself (#90):
`board/ea/common/kernel/drivers/gpu/ce5300-sgx-quiesce.c` registers a syscore
`.shutdown`. It runs after every driver's `.shutdown`, so it comes after the GPU
driver's own. It writes the reset only if a driver is still bound to the SGX.
The next driver probe releases the reset itself.

### GPU used: not recoverable without a power-cycle

Once the GPU has rendered, nothing tried brings back a clean kexec. That includes
the panel web view or any GL client, and probably the DDK's services init at
boot (`S60sgx`, retired). That last case wasn't measured separately.
None of these fixed it:

- unbinding the driver, plus the soft reset (PCI bus master was already off,
  command register `0x0002`);
- restoring every safely accessible register to its power-on value: events
  cleared, `BIF_CTRL` `0xff00` (MMU bypass), `CLKGATECTL` 0. That made it
  *worse*, with garbled reads on PCI bus 1 on top.

| SGX register | power-on (bootstrap kernel) | after rendering |
|---|---|---|
| `CLKGATECTL` (`0x0`) | 0 | `0xa6aa` |
| `CLKGATESTATUS` (`0x8`) | 0 | `0x20` |
| `EVENT_STATUS` (`0x12c`) | 0 | `0xa0004000` |
| `HOST_ENABLE` (`0x130`) | 0 | `0x4000` |
| `BIF_CTRL` (`0xc00`) | `0xff00` | 0 |

**A warm reboot through CEFDK doesn't clear it either.** The SGX keeps the used
state across the reset, and a kexec from the kernel that boots next still breaks.
Only a power-cycle clears it. Whatever state remains lies beyond the registers
that are safe to touch.

:::danger[Reading the wrong SGX register wedges the bus]
On an SGX that isn't clocked or initialised, reading `EUR_CR_USE_CODE_BASE_0`
(`0xa0c`) **hangs the whole box** and needs a power-cycle. Registers safe to
access in any state, as measured: `0x0`, `0x8`, `0x80`, `0x12c`, `0x130` and
`0xc00`. Probe only those.
:::

### The rule

**The product never kexecs out of the real kernel** (#90).

- `restore stock` stages its one-shot flag and **reboots through CEFDK**
  into boot-init. Boot-init reimages p1 from the bootstrap kernel, which never
  touched the GPU.
- `restore recovery-kexec` **refuses** while a driver is bound to
  `0000:01:02.0`.
- The flasher's `kexec` command is a developer tool, and the same limit applies
  to it.

**To RAM-test a kernel on an EA,** start from a box that has been power-cycled
since the GPU last rendered. Three ways to set that up:

- **Works on the EA3 (measured 2026-11-02, 2 of 2):** kexec at the very start of
  boot, from the bootstrap kernel, before any service has run. Put a one-shot
  script that sorts just before `S00kexec` (`/etc/init.d/S00aramboot`) on p1. It
  deletes itself, then runs `kexec -l` with the kernel, the rootfs as `--initrd`
  and `ohc.ramboot=1`. A hang recovers with a power-cycle, because the script is
  already gone.
- **Failed on the EA3, GPU never used, same image and initrd (each 1 of 1):**
  a kexec from a running system. That covers the fat kernel after unbind and
  soft reset, the fat kernel with `PVRSRVDrmInit` blacklisted, and the bootstrap
  kernel with `pvrsrvkm` never loaded, each after its services had run. So on the
  EA3 the GPU isn't the only thing that breaks the hand-off. All three showed
  the HPET symptom; EHCI `-110` was seen on the last two.
- **Proven on the EA1 with `pvrsrvkm`:** keep the outgoing kernel from binding
  the GPU at all, with `initcall_blacklist=` of the driver's init in
  `OHC_KEXEC_CMDLINE_EXTRA` in `board.env`, then power-cycle.

If a kexec'd kernel shows the HPET or DMA symptoms above, the fault is the
hand-off, not the kernel under test.
