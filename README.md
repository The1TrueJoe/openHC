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
| **hc800** | Atom D525 | proven — boots, installs to GRUB, full IO + web UI |

All eight build in CI. "Proven" means it booted on real silicon; "builds" means
exactly that and no more.

## Install on a controller

No serial cable and no button. There is a GUI and a CLI over the same engine —
use the GUI unless you are scripting.

### The GUI

Download the flasher for your OS from the
[latest release](https://github.com/The1TrueJoe/openHC/releases), open it, and
it walks you through four steps: **Find → Check → Release → Install**. It scans
the network, tells you what each unit is, and will not let you continue until it
is sure — a wrong guess flashes the wrong image at real hardware.

**Or run it straight from this repo**, no download needed:

```sh
make gui
```

That is a release build, so the first run takes a few minutes while Rust
compiles it; every run after that is seconds. What you need first:

| | |
|---|---|
| **Rust** | all platforms — [rustup.rs](https://rustup.rs), or `curl https://sh.rustup.rs -sSf \| sh` |
| **macOS** | nothing else |
| **Linux** | `libgtk-3-dev libxkbcommon-dev libssl-dev` (Debian/Ubuntu), plus `sshpass` for password logins |
| **Windows** | [PuTTY](https://www.putty.org/) for `plink.exe` — see below |

`make gui` is a convenience wrapper; this is the command it runs:

```sh
cargo run --release --manifest-path flasher/Cargo.toml -p ohc-flash-gui
```

### The CLI

Same engine, for scripting. From a release it is `ohc-flash`; from the repo:

```sh
make discover                                  # what is on the network
make flash HOST=10.0.0.42 IMAGES=openhc-ea3-v2-abc1234-dev.zip
```

`ohc-flash plan <board>` prints what an install would do, and what it would
write, without doing any of it. `ohc-flash identify <ip>` says what a unit is
and which Control4 OS version it is running.

### Logging in to a controller

Two things routinely surprise people, and neither is a fault in the tool:

- **Control4 OS 3.1.0 and later have no default root password.** It was removed.
  Nothing will guess it, because there is nothing to guess — add an SSH user with
  Composer's Network Tools and pass `--password`, or install a key. On OS 3.0.x
  and earlier, `root` / `t0talc0ntr0l4!` still works.
- **Windows cannot do password SSH on its own.** `sshpass` is a Unix program with
  no Windows build, and the OpenSSH client that ships with Windows will not take
  a password on the command line. Install PuTTY and the flasher uses `plink.exe`;
  otherwise use a key.

### Finding a controller

`make discover` lists Control4 hardware by MAC, and fills in names and models
from SDDP:

```
  ADDRESS          MAC               NAME             MODEL  VIA
* 192.168.1.172    00:0f:ff:91:63:cb Living-EA1       C4-EA1 sddp,arp
  192.168.1.139    00:0f:ff:1d:cc:b1 Front-Garage-EA1 C4-EA1 sddp,arp
  10.0.0.112       00:0f:ff:57:b9:78 -                -      arp
```

A row with no name is not a failure: SDDP announcements are periodic — one unit
here re-announces about every 80 minutes — and a controller with Director
disabled never announces at all. `ohc-flash identify <ip>` asks it directly, and
reports the Control4 OS version with it. `--all` also lists the non-Control4
devices on the SDDP bus (televisions, amplifiers, DVRs).

> **Before you install anything, read
> [Recovery](https://the1truejoe.github.io/openHC/shared/recovery/).** Every board
> has a documented path back to stock Control4, and each one depends on not having
> written to one specific region.

**The HC-800 has two install methods, and the default writes nothing at all.**
`--method kexec` starts openHC out of the running system — no partition is
opened for writing, so a power cycle returns the box to stock and there is
nothing to undo. `--method grub` is the persistent one. `ohc-flash uninstall`
reverses it.

**The IO Extender is still manual**, booting from RAM over TFTP only. See
[Installing on a controller](https://the1truejoe.github.io/openHC/build/install/).

## Going back to stock

| Board | How |
|---|---|
| EA family | press the recessed factory-restore button — it reimages kernel, rootfs and bootloader |
| CA-1 | delete `boot.scr` from the eMMC's vfat partition |
| HC-800 | `ohc-flash uninstall <ip>` — restores the `menu.lst` the install backed up |
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
packages/   Buildroot packages and the Rust workspace (iod, webd, sysmond, portal)
flasher/    the installer — Rust workspace, GUI + CLI over one engine
build/      Dockerised Buildroot
site/       this project's documentation site (Astro + Starlight)
```

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from lawfully-owned units and published GPL sources; no
vendor code is redistributed here.
