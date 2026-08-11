# openHC

Open, kernel-up firmware for the **Control4 EA1** controller (Intel Atom
CE5310, i686). This repo builds a modern Linux kernel and root filesystem from
source and boots them on the EA1 over the network — no vendor userspace, no
flash writes required to iterate.

This is a hardware research project as much as a firmware one. The bootloader,
the secure-boot straps, the netboot protocol and the recovery paths were all
reverse-engineered from a live unit and Control4's GPL disclosures; see `docs/`
for the full account.

> Scope: **EA1 only.** The EA3/EA5/HC800/HC250 share ancestry but differ in
> silicon (arch, IO MCU, audio DSP/FPGA). The tree has room for them
> (`firmware/`), but nothing here is built or tested for those boards.

## What works

- **Netboot a kernel** — CEFDK's manufacturing mode fetches a kernel over
  BOOTP+TFTP and boots it into RAM. Proven on hardware; touches no flash.
- **Unlocked bootloader shell** — reachable without the signing password.
- **Unsigned kernels boot** — `SEC_BOOT` is strapped 0 on this unit.

The known-hard part is drivers: the CE5310's eMMC and Ethernet used out-of-tree
vendor drivers that mainline does not carry. The default build sidesteps that
with an **embedded initramfs**, so a modern kernel reaches a shell on the serial
console using only the CPU and UART. Storage/network come later.

## Layout

```
board/ea1/          Buildroot BR2_EXTERNAL board tree
  configs/          ea1_defconfig
  linux/            kernel config fragment
  post-image.sh     wraps the bzImage in the CEFDK container
  cefdk-container-header.bin   real 0x580 header, reused for our kernels
build/              Dockerised Buildroot (Dockerfile + build.sh)
firmware/           secondary-processor firmware (IO MCU, FPGA, DSP) — see its README
tools/              netboot / serial / container-wrap / ssh helpers
docs/               the hardware research: boot chain, bootloader access, recovery, ...
```

## Quick start

Prerequisites: Docker, Python 3, a USB-serial adapter, and a direct Ethernet
link to the EA1. Nothing else is installed on the host.

```sh
make image          # build the netboot kernel (first run pulls Buildroot; slow)
make serial         # in one terminal: watch the console
make netboot        # in another: serve the image, then hold the ID button and
                    #   power-cycle (warm reboot keeps the link up — see docs)
```

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
