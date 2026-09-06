# openHC

Open, kernel-up firmware for **Control4 controllers**. A modern Linux kernel and
root filesystem, built from source, running on hardware the vendor shipped
locked.

📖 **[Documentation and the full hardware research →](https://the1truejoe.github.io/openHC/)**

## Supported boards

| Board | SoC | Status |
|---|---|---|
| **ea1-v1** | Intel CE5310 | proven — netboots, Wi-Fi + SSH |
| **ea3-v2** | Intel CE5310 | proven — persistent self-boot from eMMC |
| **ca1** | i.MX6 SoloLite | proven — openHC on eMMC, web dashboard |
| **ioxv1** | TI DM355 | proven — boots, Ethernet + SSH, 8 relays |
| ea1-v2, ea1-v2-poe, ea3-v1 | Intel CE5310 | build; not yet booted |
| hc800 | Atom D525 | builds; not yet booted |

## Install on a controller

The easiest path needs no serial cable and no button. Download the
[latest release](https://github.com/The1TrueJoe/openHC/releases) — the flasher
for your OS, plus the image bundle for your board — and run:

```sh
ohc-flash discover
```

```sh
ohc-flash identify <ip>
```

```sh
ohc-flash install <ip> --images openhc-<board>-<version>.zip
```

It identifies the board first and **refuses to guess** — a wrong guess flashes the
wrong image at real hardware. `ohc-flash plan <board>` prints what an install
would do without doing it.

The GUI (`ohc-flasher`) is the same engine with a front end, and is the
recommended way in if you are not scripting.

> **Before you install anything, read
> [Recovery](https://the1truejoe.github.io/openHC/shared/recovery/).** Every board
> has a documented path back to stock Control4, and each one depends on not having
> written to one specific region.

**The HC-800 and the IO Extender are manual.** The HC-800 install is two files and
a one-line edit to GRUB's `menu.lst`; the IO Extender currently boots from RAM
over TFTP only. Both are covered in
[Installing on a controller](https://the1truejoe.github.io/openHC/build/install/).

## Going back to stock

| Board | How |
|---|---|
| EA family | press the recessed factory-restore button — it reimages kernel, rootfs and bootloader |
| CA-1 | delete `boot.scr` from the eMMC's vfat partition |
| HC-800 | set `default` back to `1` in `menu.lst` |
| IO Extender | power-cycle; bring-up writes no flash |

## Build it yourself

Needs Docker and Python 3; nothing else is installed on the host.

```sh
make image BOARD=ea3-v2
```

`make help` lists the boards and prints the per-board notes. Full detail:
[Building an image](https://the1truejoe.github.io/openHC/build/).

## Layout

```
board/      Buildroot BR2_EXTERNAL root — one directory per board, plus the
            shared common/ and ea-common/ trees they compose from
packages/   Buildroot packages and the Rust workspace (ohc-webd, ohc-portal)
flasher/    the installer — Rust workspace, GUI + CLI over one engine
build/      Dockerised Buildroot
site/       this project's documentation site (Astro + Starlight)
```

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from lawfully-owned units and published GPL sources; no
vendor code is redistributed here.
