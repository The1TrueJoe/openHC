# Control4 controller hardware matrix

Evidence-based comparison for the six target controllers. `?` = not yet
verified — do not treat as fact. Sources: live EA1 pull
([`ea1-recon.md`](ea1-recon.md)), live EA3 pull ([`ea3-recon.md`](ea3-recon.md)),
live CA-1 pull ([`ca1-recon.md`](ca1-recon.md)), live HC-800 pull
([`hc800-recon.md`](hc800-recon.md)), the HC800 `sda.img`, the `.flash.config`
IO firmware manifest, the decoded per-board profile table inside the Tiva image,
and Control4 spec doc DOC-00031-C.

| | **EA-1** | **EA-3** | **EA-5** | **HC-800** | **HC-250** | **CA-1** |
|---|---|---|---|---|---|---|
| Arch | i686 | **i686** | i686 ? | **x86_64-capable**⁷ | **ARMv7** | **ARMv7** |
| SoC | Atom CE5310 | **Atom CE5310** | Atom CE ? | **Atom D525 + NM10, 2c/4t** | ARMv7 ~1 GHz | **i.MX6SL, 1x A9** |
| Board codename | ninjago | **ninjago** | garmadon² | **none**⁸ | ? | **emmet** |
| Board type strap | **1** | **2** | 3 ? | **n/a (type 0)**⁸ | n/a | **n/a (type 0)** |
| RAM | ~1.5 GB | **~1.5 GB** | ? | **2 GB** | 512 MB | **1 GB** |
| Storage | 7.6 GB eMMC | **7.6 GB eMMC** | eMMC ? | **8 GB SATA SSD + eSATA** | ~2 GB ? | **3.7 GB eMMC + 16 MB SPI-NOR** |
| Boot | CEFDK (no GRUB) | **CEFDK 36-34** | CEFDK ? | **AMI BIOS → GRUB 0.97** | U-Boot ? | **U-Boot 2014.04 (SPI-NOR)** |
| Secure boot | ? | ? | ? | **none — nothing verified**⁹ | ? | **HAB open — not enforced**⁴ |
| **IO MCU** | **TM4C1231D5** | **TM4C1231D5** | TM4C1231D5 | **LM3S1162** | **LM3S1162** | **none**⁵ |
| IO MCU UART | ttyS1 @ **460800** | **ttyS1 @ 460800** | ttyS1 ? | **ttyS3 @ 115200** | ? | **n/a** |
| MCU profile block | **2** | **3** | 0/1/4/5 ? | n/a¹⁰ | n/a | **n/a** |
| IR out (total) | **5**¹ | **7**¹ | 9 ? | 6 ?¹⁰ | 4 ? | **0** |
| IR jacks | **4** | **6** | 8 ? | 6¹⁰ | 4 ? | **0** |
| Relays | **0**³ | **1**³ | 4 ?³ | 4¹⁰ | 4 ? | **0** |
| Contacts | **0**³ | **1**³ | 4 ?³ | 4¹⁰ | 4 ? | **0** |
| RS-232 | **2 combo**¹ | **3 combo**¹ | 2 ? | **2 host UARTs (ttyS1/2)**¹¹ | ? | **1 combo 232/485**⁵ |
| Ethernet | 1x e1000 | **e1000 + BCM53125 (2 ports)** | ? | **1x RTL8168 (r8169)** | 1x | **1x FEC (RMII)** |
| **Zigbee SoC** | EM357 ? | EM357 ? | EM357 ? | EM357 ? | EM357 ? | **EM35x ?**⁶ |
| Zigbee attach | **USB-CP2104** | **USB-CP2104** | USB ? | **on-SoC UART (ttyS4)** | UART ? | **on-SoC UART (ttymxc4)** |
| Z-Wave | ? | **ZM5304** | ? | **none on-board**¹¹ | ? | **ZM5304 (ttymxc3)** |
| Wi-Fi | ath9k | **none** | ath9k ? | **USB RTL8192SU (r8712u)** | ? | **RTL8723BS (SDIO)** |
| Audio DSP | ADAU1451 | **ADAU1451** | **FPGA**² | **none** | ? | **none** |
| Analog codec | ? | **AK4621EF** | ? | **ALC888-VD (HDA)** | ? | **none** |
| Audio out | ? | **line + coax + HDMI** | ? | **2x line + coax S/PDIF**¹² | ? | **none** |
| Video out | HDMI (Intel GDL) | **HDMI (Intel GDL)** | HDMI ? | **ADV7511 + THS8200 fitted**¹³ | ? | **none (headless)** |
| GPU / graphics | PowerVR SGX (closed) | **PowerVR SGX + GC300** | PowerVR SGX | **Intel GMA 3150 / i915 (open)** | ARM SoC ? | **GC320 2D, unused** |
| **Android LXC** | **yes** | **yes** | yes | **no** | no | **no** |

¹ "IR out (total)" is the number of MCU output channels the board populates;
"IR jacks" is the rear-panel count. The difference is one internal front
blaster. Both numbers now come from the **per-board profile table decoded out of
the stock Tiva image** — see
[io-mcu-firmware.md](io-mcu-firmware.md#the-per-board-profile-table). The combo
RS-232 ports hang off the Tiva MCU and are driven with
`UART_SET_CONTROL`/`UART_SEND`/`UART_RECEIVE` — there is **no host `/dev/ttyS*`**
for them, unlike HC800's two 8250 ports. Run `iod` with `--no-serial-devices` on
EA1/EA3 until MCU-routed serial is implemented.

² The EA5's audio FPGA is why the shared "ninjago" kernel carries
`snd_ninjago_fpga*`. Its image is `garmadon-fpga-45t-spi-rev8.bin`, named in the
recovery manifest on *every* EA. **An EA3 has no FPGA** — the drivers load but
bind nothing. Do not read `lsmod` as evidence; check
`/sys/bus/*/drivers/ninjago-fpga*/` for bound devices.

³ Also read out of the Tiva profile table, from two four-pin groups
(`PF0/PF1/PF3/PF4` and `PA2/PA3/PA4/PA5`). EA1 populates none of them, EA3
populates one of each, the 9-output blocks populate all eight. Which group is
relays and which is contacts is **not** established — see
[io-mcu-firmware.md](io-mcu-firmware.md#relays-and-contacts-fall-out-of-the-same-table).

⁴ i.MX High Assurance Boot is in **Open** configuration on the CA-1:
`HW_OCOTP_CFG5 = 0` (`SEC_CONFIG[1]` clear) and the U-Boot image's IVT points at
an 8 KB CSF slot that is entirely zero-filled, so a Closed part could not boot
it. Separately, `bootcmd` never calls `hab_auth_img` — the kernel is loaded with
a plain `bootz` — so the kernel and DTB are unverified regardless of the fuse.
Derivation in [ca1-recon.md](ca1-recon.md#secure-boot-hab-is-open-and-nothing-else-is-verified-either).
The other five boards have not been checked.

⁵ The CA-1 has **no companion MCU at all** — `ioserver` opens `/dev/ttymxc2`
directly. Its single rear serial port is a host i.MX UART in front of an
RS-232/RS-485 transceiver reconfigured by seven GPIOs, not an MCU-routed combo
port. It has no IR, relays or contacts: no such device nodes, no `/sys/class`
entries, and no GPIO lines for them.

⁶ Unconfirmed. On the recon unit the Zigbee NCP returned **nothing** to an
EZSP/ASH reset at 115200/57600/38400, with and without RTS/CTS, before and after
pulsing `zigbee_reset`. EM357 is inferred from Control4's use of it everywhere
else, not measured.

⁷ The D525 reports `lm` and `nx` in `/proc/cpuinfo` — it is a 64-bit part. The
vendor nevertheless ships a **32-bit non-PAE** kernel ("NX protection cannot be
enabled: non-PAE kernel!"), which strands 1149 MB of the 2 GB in HIGHMEM and
turns NX off. openHC builds this board x86_64; it is the only board in the tree
where we deliberately do not match the vendor's architecture.

⁸ There is **no Lego codename** for this generation. `/proc/c4board/name`,
the filesystem labels, the kernel version string and `.flash.config` all just
say `hc800`. (Earlier revisions of this table said `sherman`; nothing on a live
unit uses that name.) `type` is 0, and the three GPIO straps that the EA family
would use for a board type instead carry the **board revision** — they read
`100b` = 4 on the recon unit, matching `/proc/c4board/revision`.

⁹ Nothing in the HC-800 boot chain is verified: AMI BIOS → GRUB 0.97 → a bare
`bzImage` named in a plain-text `menu.lst` on an ext3 partition. No signing, no
container format, no measured boot. It is the **least locked-down boot chain in
this table**, and the reason openHC installs on this board by copying two files
and adding a menu entry.

¹⁰ Not decoded, and the IR figure is **actively disputed**. The HC-800/HC-250
run a different MCU image (`IoProcMultiConfig1162`) from the EA family's Tiva
image and no profile table has been pulled out of it. The counts here are the
**rear-panel counts reported by the owner of the recon unit**, which is why they
carry no bold. Relays and contacts agree with the decoded Tiva table's 4+4 for
the nine-output blocks; **IR does not** — `io-mcu-firmware.md` reads the HC800
image as four IR channels from its use of TIMER0–3, against six jacks on the
panel. Those are reconcilable (each Stellaris GPTM has two CCP outputs, so four
timers can drive eight channels), but until the pin/exception table is decoded
the way the EA one was, neither number is firmware evidence. The IR *total* is
assumed equal to the jack count because no internal front blaster is known on
this chassis.

¹¹ Unlike the EA family's MCU-routed combo ports, the HC-800's two rear RS-232
jacks present as **host 8250 UARTs** — `ioserver` holds `/dev/ttyS1` and
`/dev/ttyS2` directly, alongside the MCU on `ttyS3`. Note this conflicts with
`io-mcu-firmware.md`'s reading that the LM3S image drives "the two user ports"
on its own UART1/UART2; a running system cannot tell a direct wire from a
bridge through the MCU. All five host UARTs are accounted for (console, 2x
RS-232, IO-MCU, Zigbee), which is also the evidence that there is no on-board
Z-Wave module: there is no UART left for one.

¹² From the ALC888-VD's BIOS pin defaults: line out on pin `0x14` (rear) and
`0x1b` (front), line in on `0x1a`, coax S/PDIF out on `0x1e`. Earlier revisions
of this table said the HC-800 had no audio out at all.

¹³ A TI THS8200 video DAC (SMBus `0x21`) and an ADI HDMI transmitter (SMBus
`0x72`) are **fitted and answer chip-detect**, and the vendor stack configures
them to 720p on every boot — so "headless" was wrong at the silicon level.
Whether that path terminates at a connector on this revision is untested, and
openHC builds no video for it.

## What actually varies

Across the EA/HC lineup this collapses to **two axes**. The CA-1 adds a third —
see [below](#the-ca-1-breaks-the-two-axis-model).

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

The EA1→EA3 pull confirmed this harder than expected: **the two boards are the
same computer.** Same CE5310, same 1.5 GB, same eMMC partitioning, same CEFDK,
same recovery payload shape, and *byte-identical IO-MCU firmware*. The whole
delta is peripherals — an extra IR pair, a third combo serial port, a relay and
a contact, the BCM53125 switch, and the analog/coax audio chain. That is a board
profile, not a port.

### The CA-1 breaks the two-axis model

The EA1→EA3 pull suggested the whole lineup was one computer with a board
profile bolted on. The CA-1 pull says that generalisation stops at the EA
family. It is a **genuinely different machine**, and it moves in three ways at
once:

1. **A third silicon lane.** Freescale i.MX6 SoloLite — one Cortex-A9, 1 GB,
   ARMv7. HC250 is also ARMv7, so this is not a new *toolchain* axis, but it is
   a completely different SoC with a different boot chain and no shared driver
   surface with anything else here.
2. **No IO MCU, and no IO.** Every other board delegates IR/relays/contacts/
   combo-serial to an LM3S or TM4C over the same DLE/STX framing. The CA-1 has
   none of that hardware and no MCU to run it; `ioserver` drives a host UART
   directly. The "one `iod` and one flasher for both MCU families" conclusion
   still holds — the CA-1 simply is not a member of either family.
3. **A boot chain we can actually use.** Stock U-Boot 2014.04 out of SPI-NOR,
   unlocked console, writable environment, HAB open, and a `bootcmd` that
   sources `boot.scr` off the eMMC vfat partition *before* falling back to the
   stock kernel. No CEFDK container, no signed-image problem, no netboot
   choreography — copying three files onto a FAT partition takes over the boot,
   and deleting one of them puts it back.

The practical consequence for this tree: the CA-1 was the **cheapest board to
bring up** and the one least likely to need out-of-tree kernel work, because
mainline supports i.MX6SL outright. It is also the board that most argues
against modelling everything as "a field in a board profile" — see
[ca1-recon.md](ca1-recon.md#what-this-means-for-the-port). The HC-800 pull has
since taken the "cheapest" title off it, for the reason below.

### The HC-800 breaks it hardest: it is not an embedded board at all

The CA-1 is a different SoC. The HC-800 is a **different category of machine** —
a small x86 PC with Control4 peripherals bolted to its LPC bus:

* **Lite-On motherboard, AMI BIOS, SMBIOS 2.6, DMI strings.** No other board
  here has a BIOS, let alone a field-flashable one (`/etc/init.d/flash-bios`).
* **Ordinary PC silicon end to end** — Atom D525 + NM10, AHCI SSD, ICH7 HD
  audio, ICH GPIO, i801 SMBus, iTCO watchdog, CMOS RTC, four UHCI + one EHCI.
  **openHC needs zero kernel patches for this board.** Every other board in the
  tree needed either SoC resurrection (DM355), de-device-tree glue (CE5310), or
  a reconstructed DTS (i.MX6SL).
* **A boot chain that is a text file.** GRUB 0.97 reading `menu.lst` off ext3,
  nothing signed. Installing openHC is: copy a bzImage and a cpio.gz onto the
  kernel partition, append a third menu entry, change one digit. Reverting is
  changing that digit back. Nothing else on the disk is written.
* **The Control4 parts are all on UARTs and GPIO** — an LM3S1162 on `ttyS3`, a
  Zigbee NCP on `ttyS4`, two host RS-232 ports, eight named ICH GPIO lines.
  Nothing needs a driver that does not already exist.

So the axis this board adds is not silicon or IO — it is **how much of the
machine we have to build at all**. See
[hc800-recon.md](hc800-recon.md#what-this-means-for-the-port).

### The one new subsystem: the BCM53125

EA3 puts a **managed 5-port Broadcom switch on SPI** (`spi0.1`) behind an e1000
MAC running in "internal fake phy" mode. This is the only EA3 peripheral with no
EA1 counterpart, and the only one where mainline is *better* placed than the
vendor: `spi-bcm53125` is out-of-tree, but mainline's **DSA `b53` driver**
supports the BCM53125 including an SPI binding. It needs a board description,
which is the same de-DT problem `board/ea-common/patches/linux/0001-i2c-pxa-*` already
solves for I²C.

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

Closed by the EA3 pull: EA3 SoC/RAM/arch/storage/boot, EA3 IR + serial counts,
EA3 Zigbee transport, and — via the decoded profile table — EA1's IR output
count (5, not 4: four jacks plus an internal blaster).

Closed by the HC-800 pull ([hc800-recon.md](hc800-recon.md)): its SoC, RAM,
storage and partition layout; the boot chain end to end (AMI BIOS → GRUB 0.97,
nothing verified); the full UART map including that its two RS-232 ports are
host UARTs rather than MCU-routed; the Zigbee attach point; the Wi-Fi part
(RTL8192SU on USB, not "USB Realtek ?"); the ALC888-VD audio path, which the
table previously recorded as no audio at all; and that the board has no Lego
codename. It also corrected two outright errors: `sherman` as a codename, and
"none (headless)" for video.

Still `?`:

* **EA5**: a live pull. Which profile block it uses (0/1/4/5 all describe nine
  IR outputs and two user UARTs, so the block→board map is not yet 1:1), and
  confirmation that `garmadon` is its codename rather than just its FPGA's.
* **The MCU's board-ID strap voltage.** The selector is now fully disassembled
  (`board id = round(adc/250)` on ADC0/AIN10, PB4), which *predicts* ~0.40 V for
  an EA1 and ~0.60 V for an EA3 — but nobody has measured it. Confirm and the
  IO-MCU firmware can go back to a single image for all boards. See
  io-mcu-firmware.md.
* **HC250**: boot chain, Zigbee node, arch-specific image lane.
* **The LM3S1162 profile table.** Whether `IoProcMultiConfig1162` carries a
  per-board table like the decoded Tiva image, which would put the HC-800's
  6 IR / 4 relay / 4 contact counts on firmware evidence instead of a panel
  count, and would give the HC-250's counts for free.
* **The HC-800's LED GPIO map.** Six `leds-gpio` LEDs whose ICH offsets live
  only in the vendor board file; not recoverable from a running system.
* **Whether the HC-800's ADV7511/THS8200 chain reaches a connector**, and
  whether its BIOS can boot USB — which would give a second install path that
  writes nothing at all.
* **EA3 rear-jack ↔ IR channel index mapping.**
* **Which of the two four-pin MCU groups is relays vs contacts.**
* **EA3's second switch port.** Port 5 (CPU) and port 2 (a jack) are measured;
  the other jack is inferred to be port 1.
* **Relay/contact transport on EA3** — whether `ioserver` drives its 1 relay /
  1 contact over the MCU protocol or through a host file descriptor. Its strings
  contain both paths.

Closed by the second EA3 pass: the AK4621 control path (there is none — no
kernel symbol, no module, no process holding a spidev; the part is strapped
standalone and only needs `codec_reset` released), the BCM53125 CPU port
(**5**, not the IMP at 8), and the MCU board-ID selector arithmetic.
