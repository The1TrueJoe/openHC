# openHC

Open, kernel-up firmware for **Control4 EA-series** controllers (Intel Atom
CE5310, i686). This repo builds a modern Linux kernel and root filesystem from
source and boots them over the network — no vendor userspace, no flash writes
required to iterate.

This is a hardware research project as much as a firmware one. The bootloader,
the secure-boot straps, the netboot protocol and the recovery paths were all
reverse-engineered from live units and Control4's GPL disclosures; see `docs/`
for the full account.

## Board support

| board | Wi-Fi | switch | PoE | secure boot | status |
|---|---|---|---|---|---|
| **ea1-v1** | ath9k | — | — | clear | **proven** — netboots, shell, Wi-Fi + SSH |
| **ea3-v2** | — | BCM53125 | yes | **fused** | **proven** — 7.1.8 via `bootlinux`, e1000 + eMMC + SSH, persistent self-boot |
| ea1-v2 | ath9k | — | — | ? | builds; boot differences unconfirmed |
| ea1-v2-poe | — | BCM53125 | yes | ? | builds, never booted |
| ea3-v1 | ath9k | BCM53125 | yes | ? | builds, never booted |
| **HC800** | — | — | — | n/a | builds, never booted; needs no kernel patches |
| CA1 / IOXv1 | see `board/` | | | | separate SoCs, own status |
| EA5 / HC250 | not supported; `firmware/` has room for them | | | | |

The EA variants differ only in peripherals and the secure-boot fuse, so they are
composed rather than copied: each board lists what it has in
`board/<board>/ohc.features` (`wifi`, `emmc`, `switch`) and the shared feature
sets in `board/ea-common/features/` supply the config.

The EA1 and EA3 turned out to be **the same computer**: same CE5310, same
1.5 GB, same eMMC layout, same CEFDK bootloader, and byte-identical IO-MCU
firmware. The whole difference is peripherals — see
[docs/hardware-matrix.md](docs/hardware-matrix.md). So EA3 support is a board
profile, not a port.

**One real difference showed up on hardware: secure boot.** The EA3 board v2's
CEFDK signature fuse is *blown*, so the eMMC normal-boot path rejects unsigned
kernels (the EA1's fuse is clear). It takes over anyway: the CEFDK shell
`bootlinux` command does not verify, and CEFDK's `script` autorun runs before
the verifying path, so a stored autoscript boots our unsigned kernel from a raw
eMMC region every power-on — no button, the CA-1 `boot.scr` trick done CEFDK's
way. `tools/ohc-ea-takeover.py` automates the whole install over one
direct-attach Ethernet link. The signed stock kernel is left in place as a
`script off` recovery. See [docs/ea3-recon.md](docs/ea3-recon.md#secure-boot-and-how-openhc-takes-the-ea3-over-anyway-proven-on-hardware).
The EA1 (board v1) needs none of this — its normal path boots unsigned already.

Two EA3 differences shape its build:

* **No Wi-Fi.** The EA1 uses ath9k as its reachability path; the EA3 has no
  radio at all, so it comes up on wired Ethernet (`e1000`, mainline).
* **A BCM53125 managed switch on SPI** behind the MAC. The port map is measured
  (CPU is switch port 5; the jacks are ports 1 and 2), and DSA's `b53` gives
  each rear RJ45 its own netdev — something the vendor firmware never exposed.
  The board glue (`ea-common/drivers/spi/spi-ea-b53-board.c`) now
  builds: its old failure was an include, not a design problem — `dsa_chip_data`
  moved to `<linux/platform_data/dsa.h>`, and only the `dsa_platform_data`
  wrapper was actually removed. **DSA's bring-up of the isolated second jack is
  still unproven on hardware**; if DSA does not attach, `S40net` falls back to
  DHCP on the master and the primary jack works as before.

## What works

- **Netboot a kernel** — CEFDK's manufacturing mode fetches a kernel over
  BOOTP+TFTP and boots it into RAM. Proven on the EA1; touches no flash.
- **Unlocked bootloader shell** — reachable without the signing password.
- **Unsigned kernels boot** — directly on `ea1-v1` (fuse clear). On `ea3-v2` the
  fuse is blown, so they go in via the CEFDK `script` autorun + `bootlinux`
  instead; either way no vendor code is involved.

The drivers that looked hardest are done: the CE5310's Ethernet needed a
fake-PHY patch (the MAC has no PHY — it is wired back-to-back to the switch),
its 128-line GPIO controller needed a driver mainline lacks, and the eMMC turned
out to need nothing at all — mainline `sdhci-pci` binds `8086:070b` by class.
So an EA3 now boots a modern kernel from its own eMMC with a persistent ext4
root, not just a RAM initramfs.

## Layout

```
board/                          Buildroot BR2_EXTERNAL root
  common/                       EVERY board: common_defconfig, post-build.sh
                                (openhc uid-0 alias), rootfs-overlay (shared
                                init incl. the board.env-driven S40net)
  ea-common/                    everything the EA family shares
    ea-common_defconfig         the EA half of the config
    features/                   composable feature sets, selected per board by
                                board/<b>/ohc.features: wifi, emmc, switch
    linux/                      common.fragment + wifi/emmc/switch fragments
    patches/linux/              i2c-pxa, pwm-ce5300, e1000 fake-phy,
                                gpio-intelce (CE5300 128-line GPIO),
                                spi-ea-b53-board (BCM53125 port map)
    post-image.sh               wraps the bzImage in the CEFDK container
  ea1-v1/  ea1-v2/  ea1-v2-poe/ EA1 variants (see Board support)
  ea3-v1/  ea3-v2/              EA3 variants
  ca1/  hc800/  ioxv1/          non-EA boards
packages/                       Buildroot packages (ohc-motd: figlet /etc/motd)
                                + the Rust workspace (ohc-webd, ohc-portal)
build/                          Dockerised Buildroot (Dockerfile + build.sh)
firmware/                       secondary-processor firmware (IO MCU, FPGA, DSP)
tools/                          netboot / takeover / serial / ioprobe helpers
docs/                           the hardware research: boot chain, recon, ...
```

Buildroot has no include mechanism for defconfigs, so `build/build.sh`
concatenates them: `common/common_defconfig` (every board) + the family base
(`ea-common`, EA only) + any feature sets the board lists in `ohc.features` +
the board's own file, fed in as `BR2_DEFCONFIG`. Kconfig takes the last assignment, so a board
extends or overrides the shared settings simply by coming after them — and each
board file stays a short list of genuine differences.

## Quick start

Prerequisites: Docker, Python 3, a USB-serial adapter, and a direct Ethernet
link to the controller. Nothing else is installed on the host.

```sh
make image BOARD=ea3-v2   # build the kernel + rootfs (Docker; first run is slow)
make netboot BOARD=ea3-v2 # serve it and boot it — then hold the ID button
```

Two commands. `make netboot` runs BOOTP and TFTP, waits for CEFDK to drop to
its unlocked shell, then drives the whole `bootlinux` sequence over the serial
console itself and streams the boot log to `output/boot-console.log`. The only
thing it cannot do is hold the **ID button** for you, so it prints a reminder
and waits.

It needs `sudo` (BOOTP and TFTP are privileged ports) and a USB-serial adapter
on the console header. Nothing touches flash — a power-cycle returns the unit
to stock Control4.

`BOARD` defaults to `ea3-v2` and selects the defconfig, kernel fragments, rootfs
overlay, image name, MCU firmware profile, and the netboot addressing together.

To get a bootloader shell instead of booting a kernel: `make probe`, then the
same ID-button power-cycle.

## The build in one paragraph

Buildroot (pinned LTS, run in a container) cross-compiles a musl toolchain, a
modern kernel, and a busybox initramfs into a single `bzImage`. `post-image.sh`
prepends the CEFDK container header so the bootloader will load it.
`tools/netboot.py` answers CEFDK's manufacturing-mode BOOTP with the `C4_COOKIE`
magic and serves the image over TFTP. See `docs/bootloader-access.md` for why
each of those steps is shaped the way it is.

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from a lawfully-owned unit and published GPL sources; no
vendor code is redistributed here.
