#!/usr/bin/env bash
# SGX530 vendor-stack capture: build a RAM-bootable OMAP3630 uImage with the
# openpvrsgx 1.14.3699939 module built in and a capture patch applied.
#
# Runs in GitHub-hosted CI on a wip/ branch. The module source and the TI
# user space are fetched/staged at build time and NEVER committed. Output:
# a single uImage artifact for the board's c4test.img RAM-boot path.
#
# Env the workflow sets:
#   NETCONSOLE_TARGET=<mac-ip>:<port>   (log sink; baked into the cmdline)
#   CROSS=arm-linux-gnueabihf-
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=${WORK:-$PWD/build}
mkdir -p "$work" && cd "$work"

# 1. Kernel + module tree (6.0-rc7-class, the newest the module supports).
if [ ! -d linux ]; then
  git clone --depth 1 -b letux-pvrsrvkm-6.0-rc7 \
    https://github.com/openpvrsgx-devgroup/linux_openpvrsgx linux
fi
cd linux
git apply "$here/capture.patch"            # build-time only

# 2. Config: omap2plus + the capture fragment.
export ARCH=arm CROSS_COMPILE=${CROSS:-arm-linux-gnueabihf-}
make omap2plus_defconfig
./scripts/kconfig/merge_config.sh -m .config "$here/sgx530_capture.fragment"

# 3. GPL-only initramfs. assemble-initramfs.sh stages busybox (armhf static)
#    and the GPL/distro armhf runtime libs. NO TI binaries: the closed TI user
#    space is fetched from the Mac at run time (MAC_HTTP), never into CI.
MAC_HTTP="${MAC_HTTP:?set MAC_HTTP=http://<mac-ip>:<port>}" \
  "$here/assemble-initramfs.sh" "$work/initramfs"
cp -a "$work/initramfs" ./initramfs
make olddefconfig

# 4. Build: zImage + the capture DTB. The board's ethernet is a GPMC LAN9220,
#    not beagle-xm's USB NIC, so netconsole needs the board's own DT: a trimmed
#    OMAP3630 magneto DT (ci/capture.dts), compiled in-tree.
cp "$here/capture.dts" arch/arm/boot/dts/omap3630-sgx530-capture.dts
make -j"$(nproc)" zImage
make arch/arm/boot/dts/omap3630-sgx530-capture.dtb
cat arch/arm/boot/zImage arch/arm/boot/dts/omap3630-sgx530-capture.dtb > "$work/zImage-dtb"

# 5. uImage for bootm (load/entry match the board's c4test.img flow).
mkimage -A arm -O linux -T kernel -C none -a 0x80008000 -e 0x80008000 \
  -n sgx530-capture -d "$work/zImage-dtb" "$work/openhc-sgx530-capture.uImage"
echo "artifact: $work/openhc-sgx530-capture.uImage"
