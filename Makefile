# openHC — kernel-up firmware for Control4 EA-series controllers.
#
# The heavy lifting (Buildroot: cross-toolchain, kernel, rootfs) runs in Docker
# so nothing is installed on the host. See build/build.sh.
#
# BOARD selects the target. Everything downstream — the defconfig, the kernel
# fragments, the rootfs overlay, the image name, the MCU firmware profile —
# follows from it.

BOARD ?= ea1
# ea1/ea3: Intel CE5300 (x86), companion TM4C IO-MCU, CEFDK netboot.
# ioxv1:   TI DaVinci DM355 (ARM), native on-board IO, U-Boot `run tst` netboot.
# ca1:     Freescale i.MX6SL (ARM), native on-board IO, stock U-Boot boot.scr.
# hc800:   Intel Atom D525 PC (x86_64), companion LM3S1162 IO-MCU, GRUB 0.97.
BOARDS := ea1 ea3 ioxv1 ca1 hc800

ifeq ($(filter $(BOARD),$(BOARDS)),)
  $(error BOARD must be one of: $(BOARDS)  (got '$(BOARD)'))
endif

IMAGE := output/images/openhc-$(BOARD)-kernel.img

# netboot/serial need pyserial. Homebrew and most distro pythons are
# PEP 668 "externally managed", so `pip install --user pyserial` is refused and
# there is nowhere to put it. Prefer a gitignored project venv when one exists:
#
#   python3 -m venv .venv && .venv/bin/pip install pyserial
#
# sudo runs this interpreter directly, so the venv works under sudo too.
PYTHON ?= $(if $(wildcard .venv/bin/python),.venv/bin/python,python3)

# --- netboot ----------------------------------------------------------------
# Per-board addressing (this host's IP on the controller's segment, the
# controller's MAC, the address to offer it, the console baud) lives in ONE
# place: the BOARDS table in tools/netboot.py, selected with --board. Any of it
# can still be overridden here, e.g.
#
#   make netboot BOARD=ea3 IFACE_IP=192.168.1.155
#
# The MAC matters more than it looks: the responder answers only that MAC, so a
# wrong one means it silently ignores every request and the console just says
# "Bootp configuration failed".
NETBOOT_ARGS = --board $(BOARD) \
               $(if $(IFACE_IP),--iface-ip $(IFACE_IP)) \
               $(if $(CLIENT_MAC),--client-mac $(CLIENT_MAC)) \
               $(if $(OFFER_IP),--offer-ip $(OFFER_IP)) \
               $(if $(SERIAL_PORT),--serial-port $(SERIAL_PORT))

.PHONY: help image mcu netboot probe serial clean distclean

help:
	@echo "openHC — Control4 EA-series kernel-up build"
	@echo ""
	@echo "  make image [BOARD=...]       build the netboot kernel image (Docker Buildroot)"
	@echo "  make mcu   [BOARD=ea1|ea3]   build the TM4C IO-MCU firmware (EA only)"
	@echo "  make netboot                 serve $(IMAGE) to the target (EA CEFDK path)"
	@echo "  make probe                   drop CEFDK to its shell (cookie, no kernel)"
	@echo "  make serial                  attach to the serial console"
	@echo "  make clean                   remove build output (keeps downloads)"
	@echo "  make distclean               remove everything generated"
	@echo ""
	@echo "  current BOARD=$(BOARD)   (supported: $(BOARDS))"
	@echo ""
	@echo "  EA1 is the proven board. EA3 is built from recon, not yet booted."
	@echo "  ioxv1 (DM355 IO Extender) is pre-boot: rootfs builds, but the kernel"
	@echo "  needs the DM355 resurrection patches — see docs/kernel-7.1-port.md."
	@echo "  ioxv1 netboots from U-Boot ('run tst' over TFTP), not 'make netboot'."
	@echo "  ca1 (i.MX6SL) is pre-boot but needs no SoC patches — mainline covers"
	@echo "  the silicon. It boots via a boot.scr copied onto the eMMC vfat"
	@echo "  partition; 'make image BOARD=ca1' prints the install steps."
	@echo "  hc800 (Atom D525) is pre-boot and needs no patches either — it is a"
	@echo "  PC. It boots from stock GRUB 0.97 via a third menu.lst entry;"
	@echo "  'make image BOARD=hc800' prints the install steps."

# JOBS overrides Buildroot's parallelism. build.sh already caps it by the VM's
# RAM (2.5 GB/job), but set JOBS=1 if the toolchain still gets OOM-killed —
# Docker Desktop defaults to a small VM and gcc's big files are memory-hungry.
image:
	DOCKER_BUILDKIT=1 docker build -f build/Dockerfile --target artifacts \
		--build-arg BOARD=$(BOARD) $(if $(JOBS),--build-arg BR2_JLEVEL=$(JOBS)) \
		--output type=local,dest=output/images .

# menuconfig/linux-menuconfig need an interactive container; the cache-mount
# build model builds non-interactively. Edit the board defconfigs + the linux
# fragments instead, or run buildroot menuconfig by hand against output/ if you
# need it.

# IO-MCU firmware builds with an arm-none-eabi toolchain; its own Makefile
# documents the prerequisites. The board profile is compile-time — see
# firmware/io-mcu/tm4c1231d5/include/board_profile.h.
mcu:
	@case "$(BOARD)" in \
	  ea*) : ;; \
	  hc800) echo "mcu: hc800's IO-MCU is a Stellaris LM3S1162, not a TM4C1231D5."; \
	         echo "  Same DLE/STX framing and the same TI serial flash-loader, but a"; \
	         echo "  different part and a different image — there is no openHC firmware"; \
	         echo "  for it yet. See docs/io-mcu-firmware.md and docs/hc800-recon.md."; \
	         exit 1 ;; \
	  *) echo "mcu: $(BOARD) has native on-board IO — no companion MCU to build"; exit 1 ;; \
	esac
	$(MAKE) -C firmware/io-mcu/tm4c1231d5 fw BOARD=$(BOARD)

# Serve the built kernel over BOOTP+TFTP. Needs root (binds :67/:69); run the
# printed command yourself if make cannot get privileges. EA/CEFDK only — the
# DM355 boards netboot from their own U-Boot ('run tst' over TFTP).
# One command: serves BOOTP + TFTP, waits for the CEFDK shell, then drives the
# whole bootlinux sequence over the serial console and streams the boot log.
# You hold the ID button; it does the rest.
netboot:
	@case "$(BOARD)" in \
	  ea*) : ;; \
	  ca1) echo "netboot: ca1 boots from its own U-Boot, not this CEFDK path."; \
	       echo "  Preferred: copy boot.scr + kernel + dtb onto the eMMC vfat partition"; \
	       echo "  ('make image BOARD=ca1' prints the exact steps). Its U-Boot also has"; \
	       echo "  'run netboot' / 'run loadtftp' if you would rather serve over TFTP."; \
	       exit 1 ;; \
	  hc800) echo "netboot: hc800 boots from stock GRUB 0.97 on sda1 — no netboot path."; \
	         echo "  Copy the kernel + initrd onto sda3 and add a third menu.lst entry"; \
	         echo "  ('make image BOARD=hc800' prints the exact steps). Both vendor"; \
	         echo "  entries and the factory-restore partition are left untouched."; \
	         exit 1 ;; \
	  *) echo "netboot: $(BOARD) netboots from U-Boot ('run tst' over TFTP), not this CEFDK path"; exit 1 ;; \
	esac
	@test -f output/images/bzImage || { echo "no image yet — run 'make image BOARD=$(BOARD)'"; exit 1; }
	@$(PYTHON) -c "import serial" 2>/dev/null || { \
	  echo "boot mode drives the console, so it needs pyserial, and $(PYTHON) has none."; \
	  echo "    python3 -m venv .venv && .venv/bin/pip install pyserial"; \
	  echo "  (the Makefile picks .venv up automatically, including under sudo)"; exit 1; }
	sudo $(PYTHON) tools/netboot.py $(NETBOOT_ARGS) boot

# Just get to the unlocked shell and stop there (no kernel served).
probe:
	@case "$(BOARD)" in ea*) : ;; *) echo "probe: CEFDK path is EA-only"; exit 1 ;; esac
	sudo $(PYTHON) tools/netboot.py $(NETBOOT_ARGS) probe

serial:
	@$(PYTHON) -c "import serial" 2>/dev/null || { \
	  echo "serial-console.py needs pyserial, and $(PYTHON) has none:"; \
	  echo "    python3 -m venv .venv && .venv/bin/pip install pyserial"; exit 1; }
	$(PYTHON) tools/serial-console.py $(if $(SERIAL_PORT),--port $(SERIAL_PORT)) --listen 3600

clean:
	rm -rf output/build

distclean:
	rm -rf output dl
