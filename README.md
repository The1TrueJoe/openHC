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

| | status |
|---|---|
| **EA1** | **proven on hardware** — netboots, reaches a shell, Wi-Fi + SSH up |
| **EA3** | **builds, never booted** — configured from a live recon, untested |
| **HC800** | **builds, never booted** — configured from a live recon; needs no kernel patches |
| EA5 / HC250 | not supported; `firmware/` has room for them |

The EA1 and EA3 turned out to be **the same computer**: same CE5310, same
1.5 GB, same eMMC layout, same CEFDK bootloader, and byte-identical IO-MCU
firmware. The whole difference is peripherals — see
[docs/hardware-matrix.md](docs/hardware-matrix.md). So EA3 support is a board
profile, not a port. What it is *not* is verified: nothing in the EA3 lane has
been run on hardware yet.

Two EA3 differences shape its build:

* **No Wi-Fi.** The EA1 uses ath9k as its reachability path; the EA3 has no
  radio at all, so it comes up on wired Ethernet (`e1000`, mainline).
* **A BCM53125 managed switch on SPI** behind the MAC, **not yet driven**. The
  port map is measured (CPU is switch port 5; the jacks are ports 1 and 2), and
  the goal is DSA's `b53` so each rear RJ45 becomes its own netdev — something
  the vendor firmware never exposed. The board glue is parked as
  `board/ea3/patches/linux/0003-*.patch.disabled`: it does not compile against
  7.1.8, because the legacy DSA platform-data path is gone and the port map has
  to be rebuilt as a software-node graph. So EA3 currently boots on plain
  `e1000`, which reaches the network through the **primary jack only** — the
  second one is isolated by the stock switch configuration and stays dark until
  the driver lands.

## What works

- **Netboot a kernel** — CEFDK's manufacturing mode fetches a kernel over
  BOOTP+TFTP and boots it into RAM. Proven on the EA1; touches no flash.
- **Unlocked bootloader shell** — reachable without the signing password.
- **Unsigned kernels boot** — `SEC_BOOT` is strapped 0 on the EA1 tested.
  **Not read on an EA3** — do not assume it carries over.

The known-hard part is drivers: the CE5310's eMMC and Ethernet used out-of-tree
vendor drivers that mainline does not carry. The build sidesteps that with an
initramfs rootfs, so a modern kernel reaches a shell using only the CPU, the
UART and the NIC. Storage comes later.

## Layout

```
board/                          Buildroot BR2_EXTERNAL root
  ea-common/                    everything the EA family shares
    ea-common_defconfig         the shared half of the Buildroot config
    linux/common.fragment       base kernel config delta
    patches/linux/              i2c-pxa de-DT, pwm-ce5300
    post-image.sh               wraps the bzImage in the CEFDK container
    rootfs-overlay/             ohc services, avahi, init
  ea1/
    ea1_defconfig               EA1 deltas only
    linux/ea1.fragment          ath9k
    rootfs-overlay/             Wi-Fi bring-up
    cefdk-container-header.bin  real 0x580 header, extracted from a stock EA1
  ea3/
    ea3_defconfig               EA3 deltas only
    linux/ea3.fragment          no radio (DSA block parked, see above)
    patches/linux/              EA3-only switch glue, currently .disabled
    rootfs-overlay/             wired bring-up
  hc800/                        standalone — an Atom D525 PC, not an EA
    hc800_defconfig             the whole config; no family base
    linux/hc800.fragment        5 UARTs, r8169, r8712u, ALC888, gpio_ich
    post-image.sh               publishes bzImage + initrd and the menu.lst stanza
    rootfs-overlay/             wired bring-up, GPIO/UART map in board.env
build/                          Dockerised Buildroot (Dockerfile + build.sh)
firmware/                       secondary-processor firmware (IO MCU, FPGA, DSP)
tools/                          netboot / serial / container-wrap / ssh helpers
docs/                           the hardware research: boot chain, recon, ...
```

Buildroot has no include mechanism for defconfigs, so `build/build.sh`
concatenates `ea-common_defconfig` with the board's own file and feeds the
result in as `BR2_DEFCONFIG`. Kconfig takes the last assignment, so a board
extends or overrides the shared settings simply by coming after them — and each
board file stays a short list of genuine differences.

## Quick start

Prerequisites: Docker, Python 3, a USB-serial adapter, and a direct Ethernet
link to the controller. Nothing else is installed on the host.

```sh
make image BOARD=ea3      # build the kernel + rootfs (Docker; first run is slow)
make netboot BOARD=ea3    # serve it and boot it — then hold the ID button
```

Two commands. `make netboot` runs BOOTP and TFTP, waits for CEFDK to drop to
its unlocked shell, then drives the whole `bootlinux` sequence over the serial
console itself and streams the boot log to `output/boot-console.log`. The only
thing it cannot do is hold the **ID button** for you, so it prints a reminder
and waits.

It needs `sudo` (BOOTP and TFTP are privileged ports) and a USB-serial adapter
on the console header. Nothing touches flash — a power-cycle returns the unit
to stock Control4.

`BOARD` defaults to `ea1` and selects the defconfig, kernel fragments, rootfs
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
