# Control4 controller hardware matrix

Evidence-based comparison for the five target controllers. `?` = not yet
verified — do not treat as fact. Sources: live EA1 pull
([`ea1-recon.md`](ea1-recon.md)), the HC800 `sda.img`, the `.flash.config` IO
firmware manifest, and Control4 spec doc DOC-00031-C.

| | **EA-1** | **EA-3** | **EA-5** | **HC-800** | **HC-250** |
|---|---|---|---|---|---|
| Arch | i686 | i686 ? | i686 ? | x86 (i586 userland) | **ARMv7** |
| SoC | Atom CE5310 | Atom CE ? | Atom CE ? | Atom D525 | ARMv7 ~1 GHz |
| Board codename | ninjago | ninjago ? | ninjago ? | sherman | ? |
| RAM | ~1.5 GB | ? | ? | 2 GB | 512 MB |
| Storage | 7.6 GB eMMC | eMMC ? | eMMC ? | SATA disk | ~2 GB ? |
| Boot | GRUB | GRUB ? | GRUB ? | GRUB legacy | U-Boot ? |
| **IO MCU** | **TM4C1231D5** | TM4C1231D5 | TM4C1231D5 | **LM3S1162** | **LM3S1162** |
| IO MCU UART | ttyS1 @ **460800** | ttyS1 ? | ttyS1 ? | ttyS3 (ttySStellaris) | ? |
| IR out | **4**¹ | 4 ? | 4 ? | 4 ? | 4 ? |
| Relays | 0 ? | ? | ? | 4 ? | 4 ? |
| Contacts | 0 ? | ? | ? | 4 ? | 4 ? |
| RS-232 | 2 combo¹ | 1 ? | 2 ? | 2 (ttyS1/2) | ? |
| **Zigbee SoC** | EM357 ? | EM357 ? | EM357 ? | EM357 | EM357 ? |
| Zigbee attach | **USB-CP2104** | USB ? | USB ? | on-SoC UART (ttyS4) | UART ? |
| Wi-Fi | ath9k | ath9k ? | ath9k ? | USB Realtek ? | ? |
| Video out | HDMI (Intel GDL) | HDMI ? | HDMI ? | none (headless) | ? |
| GPU / graphics | PowerVR SGX (closed) | PowerVR SGX | PowerVR SGX | Intel GMA 3150 / **i915 (open)** | ARM SoC ? |
| **Android LXC** | **yes** | yes | yes | no | no |

¹ EA1 has **4 IR jacks** (owner-confirmed), **2** of which double as serial
ports — the running MCU emits exactly two `UART_RECEIVE` port frames at startup,
and `ioserver` opens two user-serial sockets (5101/5102). They hang off the Tiva MCU and are driven with the MCU's
`UART_SET_CONTROL`/`UART_SEND`/`UART_RECEIVE` opcodes — there is **no host
`/dev/ttyS*`** for them, unlike HC800's two 8250 RS-232 ports. Run `iod` with
`--no-serial-devices` on EA1 until MCU-routed serial is implemented.

## What actually varies

The lineup collapses to **two axes**:

1. **IO MCU family** — `LM3S1162` (HC800, HC250) vs `TM4C1231D5` (EA1/3/5).
   Both are TI Cortex-M, both speak the **same** DLE/STX app framing and the
   **same** TI serial flash-loader. So `iod` and the flasher are one build; only
   the firmware `.bin` filenames and the UART node differ — i.e. profile data.
   The one HC800 config (`/control4/firmware/io/.flash.config`) enumerates all
   six device types, proving one IO server drives both MCU families.

2. **CPU arch** — x86 for everything except **HC250 (ARMv7)**. This is the only
   genuine second toolchain / image lane, and the only place `gui-remoted`'s
   musl `ioctl`/`timeval` caveats can bite.

Everything else — relay/contact/IR counts, serial port count, Zigbee transport,
the ALSA map, whether an Android UI ships — is a **field in a board profile**,
not a code path. Board-varying values live in one `case` statement in
`overlay/install.sh`; everything else in this table is reference material.

## Zigbee, concretely

All boards use an **EM357 EmberZNet NCP** (Silicon Labs). HC800/HC250 wire it to
an on-SoC UART (`/dev/ttyS4` → `/dev/ttySZigbee`); EA1 puts it behind a **CP2104
USB-UART** (`10c4:ea60` → `/dev/ttyUSB0` → `/dev/ttySZigbee`). Either way the NCP
is the same silicon and the same EZSP/ASH protocol, flashed with an `.ebl` over
ZMODEM (`zap` + `/sbin/lsz`). One `zigbeed` (EZSP over serial) covers all boards;
the only difference is which device node it opens — profile data again.

## The video stack: GPL modules, proprietary userspace, pinned to 3.12

EA HDMI runs through Intel's **CEFDK / GDL** path: `ismd*`/`pd_hdmi`/`gdl_server`
kernel modules + PowerVR (`pvrsrvkm`) + `gdl_udaemon`, with the Android
gralloc/SurfaceFlinger rendering onto the GDL plane. There is no DRM/KMS.

Nuance that matters for an open image (module licenses read off the live unit):

- **The kernel modules are GPL** (`gdl_server`/`pd_hdmi`/`ismd*` = Dual BSD/GPL,
  `pvrsrvkm` = Dual MIT/GPL) — redistributable, ideally from Intel/Control4 GPL
  drops. So the *kernel side* of HDMI is open/shippable.
- **What's proprietary is userspace**: the PowerVR GLES/EGL `.so` driver (inside
  the Android container) and Control4's Android build itself.
- They are **welded to the stock 3.12 kernel** — a mainline kernel has no driver
  for this GPU/video, so a fully-mainline image loses HDMI.

Consequence: **Buildroot + HDMI + Android UI is achievable** by keeping the 3.12
kernel + GPL graphics modules + our userspace; only the in-container GLES blob and
Control4's AOSP are non-open. A fully-open mainline video stack is a research
problem, out of scope for bring-up. Details and the two Android-UI options in
[recovery.md](recovery.md).

## Open questions to close on hardware

Per-model, still `?`: exact relay/contact/IR counts (ask the MCU:
`CAPABILITIES_GET` 0x94 on the IO UART), RS-232 port counts (`setserial -g`),
EA3/EA5 SoC + RAM + arch (a live pull), HC250 boot chain and Zigbee node.
