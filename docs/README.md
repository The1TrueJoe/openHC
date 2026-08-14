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
- [ea3-recon.md](ea3-recon.md) — the same for an EA3 rev 9: same SoC and boot
  chain as the EA1, plus the BCM53125 switch, the third combo serial port and
  the ADAU1451/AK4621 audio path. Also why there is no FPGA on this board.
- [hc800-recon.md](hc800-recon.md) — an HC-800 rev 4, and the outlier of the
  lineup: not an embedded board but a **small x86 PC** (Lite-On motherboard, AMI
  BIOS, Atom D525, SATA SSD, GRUB 0.97) with the Control4 peripherals on LPC
  UARTs and the ICH GPIO block. Nothing in its boot chain is signed, and it is
  the only board that needs **no kernel patches at all**. Includes the full
  five-UART map, the GPIO line names, the ALC888-VD jack map, and the
  `menu.lst` install path.
- [ca1-recon.md](ca1-recon.md) — a CA-1 rev 4, and a different machine entirely:
  i.MX6 SoloLite, stock U-Boot in SPI-NOR, ZM5304 Z-Wave and an EM35x Zigbee NCP
  on native UARTs, RTL8723BS Wi-Fi on SDIO, and **no IO-MCU**. Includes the
  secure-boot finding (HAB open, nothing in the chain verified) and the
  `boot.scr` path that takes over boot without touching flash.
- [hdmi-cec.md](hdmi-cec.md) — the CEC investigation and the PIC24/8051 finding.

### Firmware / sources
- [io-mcu-firmware.md](io-mcu-firmware.md) — the IO-MCU wire protocol and the
  clean-room firmware (see also `firmware/`).
- [gpl-source.md](gpl-source.md) — what Control4's GPL drops do and do not give
  us for a from-source kernel build.
