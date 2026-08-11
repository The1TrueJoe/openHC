# openHC — kernel-up firmware for the Control4 EA1.
#
# The heavy lifting (Buildroot: cross-toolchain, kernel, rootfs) runs in Docker
# so nothing is installed on the host. See build/build.sh.

IMAGE := output/images/openhc-ea1-kernel.img
IFACE_IP ?= 192.168.1.5

.PHONY: help image menuconfig linux-menuconfig mcu netboot probe serial clean distclean

help:
	@echo "openHC — Control4 EA1 kernel-up build"
	@echo ""
	@echo "  make image            build the netboot kernel image (Docker Buildroot)"
	@echo "  make mcu              build the TM4C IO-MCU firmware"
	@echo "  make netboot          serve $(IMAGE) to the EA1 (hold the ID button)"
	@echo "  make probe            drop CEFDK to its shell (cookie, no kernel)"
	@echo "  make serial           attach to the serial console"
	@echo "  make clean            remove build output (keeps downloads)"
	@echo "  make distclean        remove everything generated"

image:
	DOCKER_BUILDKIT=1 docker build -f build/Dockerfile --target artifacts \
		--output type=local,dest=output/images .

# menuconfig/linux-menuconfig need an interactive container; the cache-mount
# build model builds non-interactively. Edit board/ea1/configs + linux.fragment
# instead, or run buildroot menuconfig by hand against output/ if you need it.

# IO-MCU firmware builds with an arm-none-eabi toolchain; its own Makefile
# documents the prerequisites.
mcu:
	$(MAKE) -C firmware/io-mcu/tm4c1231d5 fw

# Serve the built kernel over BOOTP+TFTP. Needs root (binds :67/:69); run the
# printed command yourself if make cannot get privileges.
netboot:
	@test -f $(IMAGE) || { echo "no image yet — run 'make image'"; exit 1; }
	sudo python3 tools/netboot.py --iface-ip $(IFACE_IP) serve --kernel $(IMAGE)

probe:
	sudo python3 tools/netboot.py --iface-ip $(IFACE_IP) probe

serial:
	python3 tools/serial-console.py --listen 3600

clean:
	rm -rf output/build

distclean:
	rm -rf output dl
