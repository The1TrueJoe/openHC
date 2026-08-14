# HC-800 live recon

Everything below came off a running **HC-800 (board revision 4)** over SSH as
root, on 2026-08-12, at 10.0.0.111. Same vendor default password as every other
Control4 controller.

The HC-800 is the odd one out in this tree, and in the direction that helps us:
it is not an embedded SoC board at all. It is **a small x86 PC** — Lite-On
motherboard, AMI BIOS, Intel Atom D525 + NM10, SATA SSD, GRUB 0.97 — with the
Control4 peripherals hung off ISA-range 8250 UARTs and the ICH GPIO block. Every
part of the boot chain is stock PC, nothing is signed, and the bootloader reads
a plain text config off an ext3 partition we can already mount over SSH.

## Identity

```
/proc/c4board/name          hc800
/proc/c4board/hostname      hc800
/proc/c4board/revision      4
/proc/c4board/type          0   (binary 000)
hostname                    Main-HC800-000FFF57B978
uname                       Linux 3.16.38-8.260.24 #8.260.24-hc800.2 SMP PREEMPT
                            Wed May 25 11:59:34 MDT 2022 i686
toolchain (/proc/version)   gcc 4.8.3 (crosstool-NG, "Control4 Toolchain for
                            Intel CE5300 Series" — the EA toolchain, reused)
cmdline                     root=root_fs_hc800 type=ext4 ro console=ttyS0,115200
                            quiet consoleblank=0 vt.cur_default=1
eth0 MAC                    00:0f:ff:57:b9:78
wlan0 MAC                   44:6d:57:10:c7:aa   (from the radio's EFUSE, not Control4's OUI)
```

DMI, which no other board in this tree has at all:

```
sys_vendor       Lite-On Tech.
product_name     HC800          product_version  X01
board_vendor     Lite-On Tech.  board_name       AE100
bios_vendor      American Megatrends Inc.
bios_version     0.00.17        bios_date        12/20/2011
```

**No codename.** The EA boards are `ninjago`/`garmadon`, the IOX is `hammer`,
the CA-1 is `emmet`; the HC-800 is just `hc800` everywhere — in `/proc/c4board`,
in the filesystem labels, in the kernel `#8.260.24-hc800.2` version string, and
in `.flash.config`. The Lego naming convention starts after this generation.
(The hardware matrix previously listed `sherman` for this board; nothing on the
live unit uses that name.)

### `/proc/c4board/type` is 0, and the revision comes from GPIO straps

`type` is `0` (`000`) and `revision` is `4`. The revision is not a guess — the
vendor's `/etc/init.d/gpio` exports three ICH GPIO lines named `board_id0`,
`board_id1` and `board_id2`, and they read:

```
board_id2 = 1   board_id1 = 0   board_id0 = 0     ->  100b = 4
```

which is exactly `/proc/c4board/revision`. So on this board the three straps
carry the **revision**, not a board-type selector — unlike the EA family, where
a single strap voltage into the IO-MCU's ADC picks the board type. See
[GPIO](#gpio-ich-gpio-block-and-the-vendors-line-names).

## The machine

```
Intel Atom D525 @ 1.80GHz (Pineview), family 6 model 28 stepping 10
2 cores / 4 threads, 512 KB L2, 3591 BogoMIPS
MemTotal 2060912 kB (2 GB)
```

Chipset, off `lspci`:

```
00:00.0 0600 8086:a000   Pineview host bridge
00:02.0 0300 8086:a001   GMA 3150 (IGD)
00:02.1 0380 8086:a002   IGD, second function
00:1b.0 0403 8086:27d8   ICH7 HD Audio
00:1c.0 0604 8086:27d0   ICH7 PCIe port 1
00:1c.1 0604 8086:27d2   ICH7 PCIe port 2
00:1d.0 0c03 8086:27c8   ICH7 UHCI #1
00:1d.1 0c03 8086:27c9   ICH7 UHCI #2
00:1d.2 0c03 8086:27ca   ICH7 UHCI #3
00:1d.3 0c03 8086:27cb   ICH7 UHCI #4
00:1d.7 0c03 8086:27cc   ICH7 EHCI
00:1e.0 0604 8086:2448   PCI bridge
00:1f.0 0601 8086:27bc   NM10 / ICH7 LPC
00:1f.2 0106 8086:27c1   ICH7 SATA, AHCI mode
00:1f.3 0c05 8086:27da   ICH7 SMBus (i801)
02:00.0 0200 10ec:8168   Realtek RTL8111/8168 GbE
```

Everything on that list is ordinary, decade-old, first-class mainline hardware.
There is no custom silicon anywhere in the chipset — the Control4-specific parts
are all hung off the LPC UARTs, the SMBus and the ICH GPIO block.

### The CPU is 64-bit capable and the vendor did not use it

`/proc/cpuinfo` flags include **`lm`** (long mode) and `nx`. The D525 is a
64-bit Bonnell part. The vendor nevertheless ships a **32-bit, non-PAE** kernel:

```
Notice: NX (Execute Disable) protection cannot be enabled: non-PAE kernel!
CONFIG_HIGHMEM4G=y
1149MB HIGHMEM available.   887MB LOWMEM available.
```

so their 2 GB is split across a highmem boundary and NX is off. openHC builds
this board **x86_64** instead — see
[board/hc800/hc800_defconfig](../board/hc800/hc800_defconfig). This is the only
board in the tree where we are not matching the vendor's architecture, and the
`lm` flag is the whole justification.

## Storage and the partition layout

```
ATA SanDisk SSD U100 8GB, rev 10.02.00, on ata1 (AHCI, SATA 3.0 Gbps)
sda   7824600 blocks (7.46 GiB)
```

`ahci` reports `4 slots 4 ports 3 Gbps 0x3 impl` — **two** ports populated:
`ata1` is the internal SSD, `ata2` is the rear **eSATA** jack (link down with
nothing plugged in). `ata3`/`ata4` are DUMMY.

| part | size | fs | role |
|---|---|---|---|
| `sda1` | 10.3 MB | ext3 | **GRUB 0.97** — `stage1`, `stage2`, `*_stage1_5`, `menu.lst`, `device.map`. 5.3 MB free. |
| `sda2` | 1.0 GB | ext3 | `restore_fs_hc800` — the factory-restore root, with its own `/boot/bzImage`. |
| `sda3` | 200 MB | ext3 | **kernel-only** partition for the main image: just `/boot/bzImage`. 165 MB free. |
| `sda4` | 6.4 GB | ext4 | `root_fs_hc800` — the running vendor root (615 MB used). |

The reason for the odd `sda3` is worth stating, because it is also why our
install is easy: **GRUB 0.97 cannot read ext4 extents**, so the kernel cannot
live on the ext4 root. Control4 solved that by giving the kernel its own small
ext3 partition. That partition has 165 MB free and GRUB can read every byte of
it.

## Boot chain: AMI BIOS → GRUB 0.97 → bzImage

`/boot/grub/menu.lst` on `sda1`, verbatim:

```
serial --unit=0 --speed=115200 --word=8 --parity=no --stop=1
terminal serial

support_factorydefault	1
factorydefault		0
default			1
fallback		1
timeout			0
hiddenmenu

title		HC-800 Factory Default Image
root		(hd0,1)
kernel		/boot/bzImage root=restore_fs_hc800 type=ext3 ro console=ttyS0,115200 quiet

title		HC-800 Image
root		(hd0,2)
kernel		/boot/bzImage root=root_fs_hc800 type=ext4 ro console=ttyS0,115200 quiet consoleblank=0 vt.cur_default=1
```

`device.map` is `(hd0) /dev/sda`, `(hd1) /dev/hda`.

Points that matter for the port:

* **Nothing is verified.** No secure boot, no signed kernel, no measured boot,
  no container format. GRUB `kernel` loads a bare bzImage. This is the least
  locked-down boot chain of any board in this repo — the EA family needs a
  CEFDK container and netboot choreography to get a kernel in.
* **`initrd` works.** Confirmed by string-dumping the installed `stage2`: it is
  GNU GRUB 0.97 and carries the `initrd FILE [ARG ...]` builtin and the
  `[Linux-initrd @ 0x%x, 0x%x bytes]` loader message. So an external
  `rootfs.cpio.gz` boots here, with none of the ~7 MB copy-window ceiling that
  constrains the EA image.
* **The console is already serial**, at the same 115200 as every other board,
  and GRUB itself talks to it (`terminal serial`).
* **`timeout 0` + `hiddenmenu`** means no interactive menu appears. Selecting a
  different entry means editing `default` in `menu.lst`, not catching a prompt.
* **Adding a third entry does not disturb the first two.** The factory-restore
  path stays byte-identical, so recovery is untouched.

That gives openHC a bring-up install that is three file operations over SSH,
fully reversible, and never writes the vendor rootfs — see
[board/hc800/post-image.sh](../board/hc800/post-image.sh), which prints the
exact commands.

## Serial: five 8250 UARTs in the ISA range

```
Serial: 8250/16550 driver, 5 ports, IRQ sharing enabled
ttyS0  0x3f8  irq 4    16550A
ttyS1  0x2f8  irq 3    16550A
ttyS2  0x3e8  irq 11   16550A
ttyS3  0x2e8  irq 10   16550A
ttyS4  0x2f0  irq 5    16550A
```

with `CONFIG_SERIAL_8250_NR_UARTS=5` **and** `CONFIG_SERIAL_8250_RUNTIME_UARTS=5`
in the vendor config. That second symbol is the one that bites: mainline
defaults `RUNTIME_UARTS` to 4, and `ttyS4` — the Zigbee NCP — simply does not
appear if it is left alone. Both are set in
[board/hc800/linux/hc800.fragment](../board/hc800/linux/hc800.fragment).

The map, from `/dev` symlinks and from which process holds which fd:

| tty | I/O | role | evidence |
|---|---|---|---|
| `ttyS0` | 0x3f8 | serial console @115200 | `console [ttyS0] enabled` |
| `ttyS1` | 0x2f8 | **rear RS-232 port 1** | held by `ioserver` (fd 16) |
| `ttyS2` | 0x3e8 | **rear RS-232 port 2** | held by `ioserver` (fd 18) |
| `ttyS3` | 0x2e8 | **IO-MCU (LM3S1162)** @115200 | `/dev/ttySStellaris -> ttyS3`, held by `ioserver` (fd 14) |
| `ttyS4` | 0x2f0 | **Zigbee EM357 NCP** | `/dev/ttySZigbee -> ttyS4` |

**The two rear RS-232 ports are host UARTs from userspace's point of view**, not
MCU-routed like the EA family's combo ports: `ioserver` opens `ttyS1` and
`ttyS2` directly, alongside the MCU on `ttyS3`. Anything talking serial on this
board can use a plain `/dev/ttyS*`, which the EA family cannot.

One caveat, because it is a real conflict rather than a detail:
[io-mcu-firmware.md](io-mcu-firmware.md#the-per-board-profile-table) records
that the LM3S1162 image configures **UART1/UART2 as "the two user ports"**. If
that reading is right, then either those MCU UARTs go somewhere other than the
rear jacks, or the host 8250s are bridged *through* the MCU rather than wired
straight to the transceivers. A running system cannot tell those apart — the fd
map looks identical either way. It matters for `iod` (the EA family needs
`--no-serial-devices`; whether this board does depends on the answer), so it is
recorded as unresolved rather than decided here.

`ttyS4` was sitting at 9600 when read, but nothing had it open — that is the
untouched 8250 default, not a measurement of the NCP's rate.

## IO: an LM3S1162 on ttyS3

`/control4/firmware/io/.flash.config` names this board's MCU images:

```xml
<Device type="hc800">
  <bootloader>IRBootloaderSerialLM3S1162.bin</bootloader>
  <application>700-00165_LM3S1162_IoProcMultiConfig1162_03.26.15_2.8.0.507079-fw.bin</application>
</Device>
```

`hc250` uses the **same two files**; the EA boards and `amp1` use the TM4C
images. So the split is exactly as
[io-mcu-firmware.md](io-mcu-firmware.md) describes — two MCU families, one IO
server, one flasher.

The rear panel per the owner of this unit: **6 IR outputs, 4 relays, 4 contact
inputs**, plus the two RS-232 ports above. All of the IR/relay/contact hardware
hangs off the LM3S1162; there are no host GPIO lines, no `/sys/class` entries
and no device nodes for any of it.

The relay and contact counts agree with the decoded TM4C profile table, which
predicts **4 + 4** for the nine-output blocks and calls that "the HC800/HC250
complement". The IR count does not: `io-mcu-firmware.md` records the HC800 image
as using TIMER0–3 and reads that as **four** IR channels. Six jacks and four
timers are not actually incompatible — every Stellaris GPTM has two CCP outputs,
so TIMER0–3 can carry up to eight channels — but the four-channel figure was
inferred from timer count rather than from a pin/exception table the way the
EA numbers were. **Neither number is firmware-confirmed for this board.** The
counts above are a physical panel count, and the LM3S image has not been
decoded.

The MCU's reset line is a host GPIO (`io_reset`, below), so the MCU can be
held in reset or reflashed without opening the case.

## GPIO: ICH GPIO block, and the vendor's line names

`gpio_ich` (from `lpc_ich`) registers one chip: `base 206, ngpio 50`. The vendor's
`/etc/init.d/gpio` exports eight lines and gives them names via symlinks under
`/dev/gpio`. Converting to **offsets within the chip**, which is what a modern
kernel and libgpiod want (the sysfs numbers are an artefact of a dynamic base):

| name | sysfs # | offset | dir | value when read |
|---|---|---|---|---|
| `zigbee_reset` | 206 | **0** | out | 1 (released) |
| `wlan_disable` | 220 | **14** | out | 0 |
| `lan_disable` | 221 | **15** | out | 0 |
| `reset` | 230 | **24** | in | 0 — the ID/setup button |
| `io_reset` | 239 | **33** | out | 1 (MCU released) |
| `board_id0` | 240 | **34** | in | 0 |
| `board_id1` | 244 | **38** | in | 0 |
| `board_id2` | 245 | **39** | in | 1 |

`/etc/init.d/gpio` also asserts `zigbee_reset=1` and `led_power=1` at boot.

**Mainline `gpio-ich` does not name lines**, so `gpiofind` will not work here the
way it does on the CA-1's device tree. Address these by chip label + offset. The
offsets are recorded in
[board/hc800/rootfs-overlay/opt/ohc/board.env](../board/hc800/rootfs-overlay/opt/ohc/board.env).

### LEDs — mapped by name, not yet by line

Six LED class devices exist, all from a `leds-gpio` platform device that the
vendor's board file registers:

```
c4::power (on)   c4::data   c4::network   wifi::red   wifi::yellow   wifi::blue
```

There is also a `gpio-keys-polled` platform device for the button.

**Which ICH offsets these sit on is still unknown.** `CONFIG_DEBUG_FS` is off in
the vendor kernel so `/sys/kernel/debug/gpio` does not exist, and probing by
`export` is not conclusive: 30 of the 50 lines refuse to export, which mostly
reflects ICH pins not muxed as GPIO rather than lines a driver holds. Getting
the real map means disassembling the board file out of the vendor `bzImage`.
Until then openHC drives no LEDs on this board — everything else works without
them.

## Network

* **Wired:** `10ec:8168` on `02:00.0`, driven by Realtek's out-of-tree
  `r8168 8.043.02-NAPI` (`CONFIG_R8168=y` in the vendor config). Mainline's
  `r8169` covers this device; that is what openHC uses.
* **Wi-Fi:** USB, not PCIe — `0bda:8172` on the internal EHCI root hub, an
  **RTL8192SU**. Driven by the in-tree staging driver:

  ```
  r8712u: Staging version
  usb 1-7: r8712u: USB_SPEED_HIGH with 4 endpoints
  usb 1-7: r8712u: Boot from EFUSE: Autoload OK
  usb 1-7: r8712u: MAC Address from efuse = 44:6d:57:10:c7:aa
  usb 1-7: r8712u: Loading firmware from "rtlwifi/rtl8712u.bin"
  ```

  Two useful consequences: the driver is **already in mainline staging**
  (`drivers/staging/rtl8712`), and its firmware file `rtlwifi/rtl8712u.bin` is
  shipped by Buildroot's `linux-firmware` under
  `BR2_PACKAGE_LINUX_FIRMWARE_RTL_87XX` — verified in the 2024.02.9 package's
  file list. No vendored blob and no post-build lift needed, unlike the CA-1.

  Note RTL8192SU is *not* covered by mainline's `rtl8xxxu`; staging `r8712u` is
  the only driver. If it has been removed by 7.1.8, Wi-Fi is the thing that
  breaks, and wired reachability is unaffected.

`wlan_disable` and `lan_disable` GPIOs can hard-disable either radio/NIC.

## Audio: ALC888-VD on ICH7 HD Audio

```
card 0: HDA Intel at 0xfe978000 irq 16
codec#2: Realtek ALC888-VD (0x10ec0888), subsystem 0x14a4d102
```

The BIOS pin defaults describe the rear panel exactly, so no model quirk is
needed — mainline `snd_hda_codec_realtek` will produce the right jacks:

| node | pin default | jack |
|---|---|---|
| `0x14` | `0x01044110` | **Line Out**, ext rear |
| `0x1b` | `0x02244120` | **HP Out**, ext front — the second stereo output |
| `0x1a` | `0x01843150` | **Line In**, ext rear |
| `0x1e` | `0x01441140` | **S/PDIF Out**, ext rear — the **coax digital out** |
| `0x11` | `0x18561130` | Digital Out, *internal* HDMI |

That is the owner's "2 line out, 1 line in, 1 coax out", one for one. Every
other pin complex on the codec reads `0x411111f0` (not connected).

## Video: present in silicon, absent from the panel

Two video chips are instantiated on the **SMBus** (`i2c-6`, `SMBus I801 adapter
at 0400`) by the vendor's board code:

```
i2c i2c-6: new_device: Instantiated device ths8200 at 0x21
i2c i2c-6: new_device: Instantiated device adv7511 at 0x72
ths8200 6-0021: THS8200 Chip Detect SUCCESS!
  ##-- c4_adi_7513 --##  Chip Detect SUCCESS!
  ##-- c4_vid_conf --##  Intialized to 720p
```

so a TI THS8200 video DAC and an ADI HDMI transmitter are **fitted and
responding**, and the vendor stack configures them to 720p at every boot. The
hardware matrix previously called this board "none (headless)"; that is wrong at
the silicon level.

It is still right at the panel level for our purposes — the unit's rear panel
has no video connector the owner uses, openHC is headless on every board, and
none of this is on the path to a booting image. Recorded as an open question,
not built.

Note also `i2c-0..i2c-5` are `i915 gmbus` buses; only `i2c-6` is the SMBus.

## Everything else that showed up

* **Watchdog:** `iTCO_wdt` (Intel TCO v1.11), and the vendor userspace actually
  runs `/sbin/watchdog -t 10 /dev/watchdog`.
* **RTC:** `rtc_cmos` — a normal PC CMOS RTC with alarm and 114 bytes of NVRAM.
  Every other board in this tree needed an I²C RTC hunt; this one does not.
* **Thermal:** four ACPI `cooling_device`s, `CPU0: Thermal monitoring enabled
  (TM1)`. No `hwmon` devices registered by the vendor kernel — the Atom's
  `coretemp` is simply not built in.
* **USB:** four UHCI companions plus one EHCI. The Wi-Fi radio occupies an
  internal port; one external USB jack on the panel.
* **`flash-bios` in `/etc/init.d`** — the BIOS is field-flashable from Linux on
  this board. Not touched, not needed, and worth remembering as a way to brick a
  unit.
* **`/proc/config.gz` is present**, which made the vendor's kernel config
  directly readable rather than inferred. That is where
  `SERIAL_8250_RUNTIME_UARTS=5` came from.
* **Plain `scp` does not work** against this unit, and the failure is not
  obvious: modern OpenSSH `scp` speaks SFTP, and the vendor's dropbear has no
  `sftp-server`, so it dies with `subsystem request failed on channel 0`. Use
  `scp -O` to force the legacy protocol, or pipe through the existing helper —
  `cat file | tools/ssh <ip> 'cat > /path'`. Both were verified byte-for-byte.
  (`tools/ssh` itself also races the dropbear pty handshake under `sshpass`
  roughly one connection in three and fails with "Permission denied"; it works
  on retry, and a single `ssh -o ControlMaster` session avoids it entirely.)

## What this means for the port

The HC-800 is the **cheapest board in this repo to bring up**, ahead of even the
CA-1:

1. **Every driver it needs is mainline and has been for a decade** — `ahci`,
   `r8169`, `snd_hda_intel`, `i2c_i801`, `lpc_ich`/`gpio_ich`, `iTCO_wdt`,
   `8250`, `ehci/uhci`. The only staging component is the USB Wi-Fi. There is no
   SoC resurrection work (DM355), no de-device-tree patching (CE5310), and no
   board DTS to reconstruct (i.MX6SL). **openHC needs zero kernel patches for
   this board.**
2. **The boot chain is a text file.** Add an entry to `menu.lst`, drop a bzImage
   and a cpio.gz on `sda3`, point `default` at it. Revert by changing one digit.
3. **No image-size ceiling.** GRUB is not CEFDK; the aggressive size trim in
   `ea-common/linux/common.fragment` exists to fit a ~7 MB copy window that does
   not apply here.
4. **It is a PC, so it is not a family.** Nothing about this board generalises to
   the EA/IOX/CA lines, which is why `board/hc800` is standalone with no shared
   base.

What is *not* solved by any of the above is the Control4-specific IO: the
LM3S1162 on `ttyS3` still needs the DLE/STX protocol implemented before the 6 IR
outputs, 4 relays and 4 contacts do anything. That work is shared with the EA
family and with the HC-250.

## Open questions on this board

* **The LED GPIO map** — six `leds-gpio` LEDs whose ICH offsets are only in the
  vendor board file. See [LEDs](#leds--mapped-by-name-not-yet-by-line).
* **The LM3S1162 profile table** — whether the `IoProcMultiConfig1162` image
  carries a per-board table like the decoded TM4C one. This would settle the
  **six-jacks-versus-four-timers IR conflict** with
  [io-mcu-firmware.md](io-mcu-firmware.md), and confirm the 4/4 relay/contact
  counts from firmware rather than from the panel.
* **Whether the two rear RS-232 jacks are wired to the host 8250s or bridged
  through the MCU's UART1/UART2.** Decides whether `iod` needs
  `--no-serial-devices` on this board the way it does on the EA family.
* **The Zigbee NCP's real baud rate** — never opened during this pull.
* **Whether the ADV7511/THS8200 video path terminates at a connector** on this
  revision, and whether it is reachable at all without the vendor's
  `c4_vid_conf`.
* **BIOS boot-device options** — whether USB boot is available, which would give
  a second install path that touches the SSD not at all. Requires a serial
  console at power-on.
