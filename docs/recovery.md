# Recovery: how not to brick the EA1

**Read this before writing anything to the device.** Confirmed by dumping the
EA1's SPI NOR (`/dev/mtd0ro`, 16 MB, read-only) and analyzing the bootloader.

## The headline

The bootloader is **Intel CEFDK** (Consumer Electronics Firmware Development Kit)
living in **SPI NOR — physically separate from the eMMC**. That single fact is the
safety net:

> **Destroying the eMMC does not brick the unit.** CEFDK survives in NOR and can
> re-flash the eMMC, boot a kernel over the network, or accept a new image over
> serial.

The stock rootfs, kernel, Android container — all of it lives on eMMC
(`/dev/mmcblk0`). CEFDK does not.

## Three boot modes, chosen by CEFDK at power-on

CEFDK reads GPIO buttons on every boot to decide:

```
Executing Control4 Normal Boot Mode      → kernel from eMMC, root=/dev/mmcblk0p1
Execute Control4 Recovery Kernel         → same rootfs + "recovery=1" on the cmdline
Entering Control4 Manufacturing Mode     → TFTP-netboots a kernel  ← our dev path
```

Buttons CEFDK probes (symbols in the image): `c4_restore_button_is_pressed`
(factory restore), `c4_id_button_is_pressed`. If button-GPIO setup fails it logs
and falls through to normal boot.

Manufacturing mode needs a **network cookie** or it declines:

> "Control4 Manufacturing Mode Network Cookie Not Found. Continue Normal Boot.
> Button may be depressed accidentally or user may be doing a network reset."

…then TFTPs with this cmdline shape:

```
console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 ip=%s mfgtest=1 \
  memmap=exactmap memmap=128K@128K memmap=1585M@1M vmalloc=586M \
  androidboot.hardware=intelce
```

**This is the zero-risk development path: netboot our kernel, never write eMMC.**
The cookie is **`C4_COOKIE`** in DHCP option 60 — reverse-engineered and working
(see `bootloader-access.md`).

## What the factory-restore button actually does (measured 2026-08)

Captured the full recovery-kernel log on hardware. The recessed **factory-restore
button** (GPIO 31) is not a light touch — it is a **complete stock reimage sourced
entirely from p2**, in this order:

1. `mke2fs` — **reformats p1** (yes, it runs mkfs; an earlier note here was wrong).
2. mounts p1 + p2, copies the stock rootfs and config from **p2 → p1**.
3. unpacks `/mnt/rec/recovery_kernel.deb` and `dd`s the **stock 6.7 MB kernel**
   (7,009,216 B = 0x006af3c0, the eMMC container size stored at eMMC `0x200`) back
   over the eMMC kernel region — logs `Wrote Kernel Size To Flash Success!`.
4. unpacks `/mnt/rec/cefdk.deb` and `dd`s **stock CEFDK (512 KB)** back — it even
   reflashes the bootloader.
5. reboots.

**Consequence for going persistent:** the button restores **kernel + rootfs +
CEFDK** to stock, so flashing our own kernel to the eMMC container and our rootfs
to p1 is fully reversible with one press. The single region that must **never** be
written is **p2** — it holds the entire recovery payload (`recovery_kernel.deb`,
`cefdk.deb`, the rootfs) and is the one source of truth for the reimage. Take a
full 7.6 GB eMMC backup before the first write regardless.

## The CEFDK shell

Every failure path "drops to shell" — a full bootloader shell on `ttyS0` @ 115200.
Confirmed commands relevant to recovery:

| Command | Why it matters |
|---|---|
| `emmc rd\|wr <addr> <buf> <size>` | **raw eMMC read/write** — restore a full image |
| `emmc rd_bp\|wr_bp <1\|2> …` | eMMC hardware boot partitions |
| `emmc set_boot <user\|boot1\|boot2>` | change which partition boots |
| `emmc info` / `emmc dump csd\|ext_csd` | eMMC identification |
| `spi_flash rd\|wr <addr> <buf> <size>` | read/write the NOR — including CEFDK itself |
| `tftp` | pull kernel/image over the network |
| `ymodem <buf> [port [baud]]` | serial file transfer |
| `bootlinux "<kernel cmd line>"` | boot Linux with an **arbitrary cmdline** |
| `bootkernel [-id\|-b\|-f\|-l\|-h] "<cmd>"` | boot a kernel by MFH id |
| `ramdisk <start> <length>` | supply an initrd |
| `mfh <bp1\|bp2> init\|add <type> <id> …` | manage NOR slots (Master Flash Header) |
| `Settings` | show/alter CEFDK BIOS settings |

MFH slot types present: `bootloader`, `cefdk`, `kernel`, `ramdisk`, `splash`,
`script`, `sec_fw`, `manifest`, `bl_params`, `plat_params`, `ip_params`, `psvn`,
`partition`. Note `script` = "Automatic shell script commands" — CEFDK can
autorun a stored script.

CEFDK also supports **multiple CEFDK slots** ("active or inactive cefdk in boot
partition", "add/delete cefdk in boot partition") and self-recovery over serial:

> "Please send a working CEFDK via YMODEM now..."

So even a corrupted bootloader is recoverable — with serial access.

## Recovery matrix

| What you broke | Recoverable? | How | Needs |
|---|---|---|---|
| eMMC rootfs (p1) | **yes, easily** | CEFDK shell → `emmc wr` restore, or TFTP a rescue kernel | serial |
| eMMC entirely | **yes** | same | serial |
| Per-unit `common.hcfg` on p2 | **yes** | regenerable from the eth0 MAC — see `/etc/init.d/hdmi_edid_fixup` lines 43-47 | ssh |
| CEFDK in NOR | **yes** | YMODEM a working CEFDK | serial |
| NOR *and* no serial | **no** | external SPI programmer (clip onto the flash) | hardware |

**Serial console access is the gating requirement for all of it.** Without serial
you have no CEFDK shell, and the safety net is theoretical.

## Backup status

| Item | Size | Status |
|---|---|---|
| SPI NOR (`/dev/mtd0ro`) | 16 MB | **done** — `mtd0.bin`, held locally |
| Full eMMC (`/dev/mmcblk0`) | 7.6 GB | **not yet** — do this before any write |
| `/mnt/persistent` (p3) | 32 MB | not yet |
| p2 recovery partition | 1 GB | **contents unverified** |

Backups are **your device's own data, kept local**. They contain Control4 and
Intel proprietary code and must never be committed or redistributed — `.gitignore`
excludes them. They exist so we can put your unit back exactly as it was.

## Clean-room boundary

We read stock firmware to learn **interfaces** — the boot flow, the MCU wire
protocol, GPIO names, which UART is which. We do not copy Control4 or Intel code
into anything a userspace ships. A netbooted kernel is independent
implementations written from observed protocol behavior.

The unresolved tension is the UI: keeping *Control4's* Android build is a
redistribution problem, even though it runs fine. Options, cleanest first:
1. **Build our own Android/AOSP container** targeting the same GDL interface.
2. Ship a non-Android UI on the GDL/DirectFB plane.
3. Treat the stock Android container as *the user's own existing files*, left in
   place on their unit and never redistributed by us.

(3) is fine for personal bring-up on your own box and is what the milestones
assume; it is not distributable. Decide before publishing images.

## Secure Boot / kernel signing — the showstopper question, mostly answered

**Verdict: almost certainly NOT enforced on retail EA1. Confirm with a 5-minute
serial test before betting the image plan on it.**

CEFDK *contains* a verification chain — `VERIFY_S2`, `VERIFY_S3(kernel bzImage)`,
`VERIFY_S3(initrd)`, each with `PASS` and `FAIL` variants, plus `STAGE2_AUTH` /
`STAGE2_CODE` blobs and a `manifest --> Security Manifest Table` MFH slot. So the
machinery exists. But enforcement is gated, not compiled in:

- The mode is a **fuse bit**: *"0b: Normal Boot Operation | 1b: Secure Boot
  Operation"*, and separately the Intel Manufacturing **FACR** fuse (`IM` setting:
  *"Disables Intel Manufacturing Trusted Boot using Intel FACR Key"*). Both are
  per-unit hardware state, not firmware constants.
- The **live unit reports Normal Boot**: CEFDK logs *"Executing Control4 Normal
  Boot Mode"* and the running `/proc/cmdline` carries **no** `secureboot`/
  signature flag.
- Both `PASS` and `FAIL` code paths exist at runtime — a hard-signed build would
  not need a FAIL branch. Enforcement is a runtime decision keyed off the fuse.
- **Control4's own manufacturing mode TFTP-boots an arbitrary kernel** (see
  below). That factory workflow is impossible if Trusted Boot is fused on. OEMs
  rarely blow FACR on field-updatable AV gear.

Confidence: high that we can boot our own kernel. The one thing that would make
it certain is reading the fuse — only doable from the CEFDK serial shell.

**The definitive test (needs serial, ~5 min, non-destructive):** at the CEFDK
shell, `tftp` a self-built unsigned bzImage into RAM and `bootlinux "…"` it. If it
runs, signing is off, full stop. If it dies at `VERIFY_S3(kernel bzImage): FAIL`,
signing is enforced and the plan changes to "stay in the Android-LXC-on-stock-
kernel lane."

## Can we boot our own code WITHOUT serial?

Yes — with important limits. **Serial is only needed to *recover a bad flash*.
Nothing below writes flash, so nothing below can brick the unit** (a bad boot =
power-cycle back to stock). Ranked by ambition:

| Path | Boots our… | Needs serial? | Needs button? | Writes flash? | Status |
|---|---|---|---|---|---|
| Run our binaries over SSH | userspace | no | no | no | **works today** (we have root + rw rootfs) |
| Replace init / swap userspace | userspace | no¹ | no | rootfs only (reversible) | viable; keep a recovery path first |
| **mfg-mode netboot** | **kernel + userspace** | no | **yes** | no | primary no-serial custom-kernel path |
| kexec from the running kernel | kernel | no | no | no | **BLOCKED** — stock kernel has `# CONFIG_KEXEC is not set` |
| Write our kernel to the eMMC MFH region | kernel | **yes²** | no | eMMC | needs serial as the recovery net |

¹ If our init is broken the kernel still boots but hangs in userspace; recover via
the restore-button recovery kernel or mfg netboot — so don't replace init until
one of those is confirmed. ² A bad eMMC kernel makes CEFDK "drop to shell" =
serial. Don't write the kernel region without serial.

**Key enablers discovered on the live unit:**
- Ethernet is **`e1000`** (`CONFIG_E1000=y`, in-tree mainline). So a kernel *we*
  build has working network out of the box → netconsole / ssh / a webserver give
  us feedback with **no serial cable**.
- Full stock kernel config saved: `backups/ea1/config-3.12.74.gz` — build a
  drop-in-compatible kernel (same drivers) and just add `CONFIG_KEXEC=y`,
  `CONFIG_RELOCATABLE=y`, netconsole, etc.
- kexec is the one clean no-serial-no-button path, and it's **off in the stock
  kernel**. Bootstrapping it needs one netboot of a kexec-enabled kernel first;
  after that, kernel iteration is a pure-software `kexec -e` loop.

**Strategic note for EA1 specifically — Buildroot + HDMI + Android IS doable.**
Buildroot is not the blocker; it can build an image around any kernel and package
prebuilt blobs. The real constraint is that HDMI/GPU are welded to the **stock
3.12 kernel** — a *mainline* kernel has no driver for this GPU/video path, so
"fully-from-source mainline image" loses HDMI. Keep the 3.12 kernel and it works.

Good news on licensing (confirmed via `MODULE_LICENSE` on the live unit):

| Module | Declared license |
|---|---|
| `gdl_server`, `pd_hdmi`, `ismdcore`, `ismdvidrend`, `pal_linux`, `osal_linux`, `sven_linux` | **Dual BSD/GPL** |
| `galcore` (Vivante 2D) | **Dual BSD/GPL** |
| `pvrsrvkm` (PowerVR GPU) | **Dual MIT/GPL** |
| `e1000` (ethernet) | GPL, mainline in-tree |

So **the kernel and every graphics kernel module are GPL — redistributable**, and
ideally sourced from Intel's CEFDK / Control4's GPL release rather than copied off
a unit. The proprietary surface is *only* userspace and shrinks to:
1. the **PowerVR userspace GLES/EGL driver** (Imagination proprietary `.so` — lives
   *inside* the Android container, not on the host), and
2. the **Android container itself** (`/usr/var/lib/lxc/android/rootfs`, ~674 MB) —
   Control4's AOSP build + apps, which the clean-room rule says we don't reuse.

Two ways to keep the Android UI:
- **(a) Reuse the stock container** — works immediately, but it's Control4/Imagination
  proprietary: fine on *your own* device (your own files), **not** redistributable
  and against the clean-room rule for a published image.
- **(b) Build our own AOSP container** targeting the same GDL/gralloc HAL — clean-room
  clean, but a big effort and it *still* needs the proprietary PowerVR GLES `.so`
  (extract per-device, or from an Intel CE SDK/GPL drop).

Two targets, depending on priorities — full analysis in
[openness.md](openness.md):
- **Max compatibility** (keep Android + GPU + HW video): stock 3.12 kernel + the
  graphics modules + PowerVR + Android container. Needs no custom kernel, no
  serial. Works today; carries the most blobs.
- **Minimum blobs** (owner's stated goal): **mainline i686 kernel + `simplefb`
  HDMI + a software-rendered LVGL/Qt UI**, no Android, no GPU. Essentially
  blob-free. Gated on the `simplefb` go/no-go test in openness.md.

## Netboot: the zero-risk development path (recipe)

CEFDK's manufacturing mode boots a kernel over the network and **never writes
eMMC** — ideal for iterating on our own kernel/rootfs.

Mechanism (from the NOR dump):
- Trigger: hold the mfg/recovery button at power-on **and** answer DHCP with a
  cookie. Marker string is **`C4_COOKIE`**; CEFDK sends a vendor-class DHCP
  request and looks for the cookie in the reply. Without it: *"Network Cookie Not
  Found. Continue Normal Boot."* (exact DHCP option TBD — read it off the wire
  with `tcpdump` when we first try, or from the CEFDK shell.)
- Transport: **TFTP** — `Control4 Manufacturing Mode :: tftp server(%s) file(%s)`.
  CEFDK pulls `next-server` + `bootfile` from the DHCP reply.
- Payload: an **XZ-compressed** kernel (CEFDK decompresses it; it rejects with
  *"Input is not in the XZ format (wrong magic bytes)"*).
- Cmdline CEFDK applies to the mfg kernel:
  ```
  console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 ip=%s mfgtest=1 \
    memmap=exactmap memmap=128K@128K memmap=1585M@1M vmalloc=586M \
    androidboot.hardware=intelce
  ```

To stand up our own netboot server we need: a DHCP server that hands out the
cookie + `next-server`/`bootfile`, a TFTP server, and an `xz`-wrapped bzImage +
initrd. This is milestone 5, and it lets us validate a full open kernel with
zero risk to the eMMC.

## Kernel storage (stock)

The stock kernel is **not a file on the rootfs** — `find` for `bzImage`/`vmlinuz`
comes up empty. It lives in the ~128 MB unpartitioned tail after `mmcblk0p3`
(sectors 7,372,800 → 7,634,944), a CEFDK **MFH-managed** raw region (`partition
--> Extended partitions managed by MFH`). Implication: replacing the stock kernel
means writing that MFH region, not editing a file — another reason netboot is the
sane iteration path.

## Unverified — close these before any destructive step

1. **Serial console: where is it physically on the EA1 board?** Header, pinout,
   voltage. Everything above depends on this. ← highest priority
2. How do you actually enter the CEFDK shell — keypress on serial during
   "Boot Shell Timeout", or only via a failed boot?
3. The manufacturing-mode **network cookie** format (DHCP option? TFTP file?).
   This unlocks netboot, the whole no-risk dev path.
4. ~~Is p2 a bootable factory image or just a config store?~~ — **ANSWERED**: p2
   is the recovery payload (`recovery_kernel.deb`, `cefdk.deb`, stock rootfs). See
   the factory-restore section above.
5. ~~What the factory-restore button actually does — does it reimage from p2?~~ —
   **ANSWERED**: yes, full reimage of kernel + rootfs + CEFDK from p2 (measured).
6. ~~Whether CEFDK enforces signature checks~~ — **ANSWERED**: `bootkernel`/mfg
   enforce RSA (fuse set); `bootlinux` does NOT — we boot unsigned through it.
7. The exact DHCP option carrying `C4_COOKIE` — read it off the wire the first
   time we netboot, or from the CEFDK shell.
