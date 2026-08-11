# openHC docs

The hardware research behind the build — how the EA1 boots, how to get into it,
and what the silicon actually is. Everything here was verified on a live unit
over the serial console and cross-checked against Control4's GPL disclosures.

Start here if you want to understand *why* the build works the way it does.

### Boot & access
- [boot-chain.md](boot-chain.md) — how the EA1 boots, from SPI-NOR through CEFDK
  to the kernel; netboot and custom-kernel paths.
- [bootloader-access.md](bootloader-access.md) — getting into the CEFDK shell,
  the manufacturing-mode BOOTP/`C4_COOKIE` protocol, `SEC_BOOT=0`, and how a
  netbooted kernel is loaded. **The core how-to for this repo.**
- [recovery.md](recovery.md) — the recovery nets (recovery button, initramfs
  `c4`, watchdog) and how not to brick the unit.

### Hardware
- [hardware-matrix.md](hardware-matrix.md) — the controller line at a glance;
  where the EA1 sits and how the others differ.
- [hardware-interfaces.md](hardware-interfaces.md) — per-subsystem interface
  reference and what is open vs vendor-locked.
- [ea1-recon.md](ea1-recon.md) — live recon of one unit: SoC, memory map,
  device nodes, kernel modules.
- [hdmi-cec.md](hdmi-cec.md) — the CEC investigation and the PIC24/8051 finding.

### Firmware / sources
- [io-mcu-firmware.md](io-mcu-firmware.md) — the IO-MCU wire protocol and the
  clean-room firmware (see also `firmware/`).
- [gpl-source.md](gpl-source.md) — what Control4's GPL drops do and do not give
  us for a from-source kernel build.
