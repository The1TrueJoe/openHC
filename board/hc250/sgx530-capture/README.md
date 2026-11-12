# HC-250 SGX530 vendor-stack capture (GPL harness)

Builds a RAM-bootable OMAP3630 `uImage` to capture what the vendor PowerVR init
does on this board's SGX530, to validate the open SGX530 bring-up.

What is here is **GPL only**: the kernel + the openpvrsgx module (fetched at
build time, not committed) with a capture patch, and a GPL/distro armhf
initramfs. The closed PowerVR user space is **not** built, fetched or packaged
here — it is served to the board's RAM from a developer machine at run time, so
no closed binary enters CI or this repo.

- `.github/workflows/sgx530-capture.yml` — manual (`workflow_dispatch`) build.
- `build.sh` — fetch openpvrsgx (6.0-rc7), apply `capture.patch`, configure with
  `sgx530_capture.fragment`, build zImage + `capture.dts`, package a uImage.
- `assemble-initramfs.sh` — busybox + armhf runtime libs + a run-time init that
  fetches the closed user space from the developer machine over http.
- `capture.patch` — logs the init-script register writes and dumps the uploaded
  microkernel/PDS words (named registers only; no MMIO sweep).
- `capture.dts` — trimmed magneto DT (GPMC LAN9220 for netconsole; raised
  thermal trip).

RAM boot only; never writes NAND. Board slot and the closed user space are
arranged out of band.
