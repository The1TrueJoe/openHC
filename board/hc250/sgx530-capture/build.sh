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
git apply "$here/capture-snap.patch"       # kick snapshots, build-time only

# 2. Config: omap2plus + the capture fragment.
export ARCH=arm CROSS_COMPILE=${CROSS:-arm-linux-gnueabihf-}
make omap2plus_defconfig
./scripts/kconfig/merge_config.sh -m .config "$here/sgx530_capture.fragment"

# 3. GPL-only initramfs. assemble-initramfs.sh stages busybox (armhf static)
#    and the GPL/distro armhf runtime libs. NO TI binaries: the closed TI user
#    space is fetched from the Mac at run time (MAC_HTTP), never into CI.
# 2b. The render test, built against Debian stretch's armhf glibc (2.24) so it
#     runs on the initramfs libc the vendor user space also uses.
cname=stretch-rt-$$
docker run --name "$cname" -v "$here:/src:ro" debian/eol:stretch bash -ec '
  apt-get -o Acquire::Check-Valid-Until=false update >/dev/null
  apt-get install -y --no-install-recommends gcc-arm-linux-gnueabihf libc6-dev-armhf-cross >/dev/null
  arm-linux-gnueabihf-gcc -O2 -Wall -o /render-test /src/render-test.c -ldl
  arm-linux-gnueabihf-gcc -O2 -Wall -shared -fPIC -o /drmname-shim.so /src/drmname-shim.c -ldl'
docker cp "$cname:/render-test" "$work/render-test"
docker cp "$cname:/drmname-shim.so" "$work/drmname-shim.so"
docker rm "$cname" >/dev/null
file "$work/render-test"

RENDER_TEST="$work/render-test" \
DRMNAME_SHIM="$work/drmname-shim.so" \
MAC_HTTP="${MAC_HTTP:?set MAC_HTTP=http://<mac-ip>:<port>}" \
  "$here/assemble-initramfs.sh" "$work/initramfs"
cp -a "$work/initramfs" ./initramfs
make olddefconfig

# 4. First pass: build vmlinux/zImage and the modules. The pvrsgx dir's
#    `always += pvr` builds pvrsrvkm_<soc>_<sgx>_<rev>.ko as a module.
make -j"$(nproc)" zImage modules

# 4b. Stage the omap3630 services module into the initramfs and re-pack it
#     (CONFIG_INITRAMFS_SOURCE packs ./initramfs at the zImage link).
ko=$(find drivers/gpu/drm/pvrsgx -name 'pvrsrvkm_omap3630_sgx530_125.ko' | head -1)
[ -n "$ko" ] || { echo "ERROR: pvrsrvkm omap3630 module not built"; find drivers/gpu/drm/pvrsgx -name '*.ko'; exit 1; }
cp "$ko" ./initramfs/pvrsrvkm.ko
echo "staged module: $ko ($(du -h ./initramfs/pvrsrvkm.ko | cut -f1))"
make -j"$(nproc)" zImage   # re-link zImage, re-packing the initramfs with the .ko

# 5. Build the capture DTB directly with cpp + dtc (kbuild's per-dtb target path
#    differs by kernel version; this is version-independent). The board's
#    ethernet is a GPMC LAN9220, not beagle-xm's USB NIC, so netconsole needs the
#    board's own DT: a trimmed OMAP3630 magneto DT (ci/capture.dts).
cp "$here/capture.dts" arch/arm/boot/dts/omap3630-sgx530-capture.dts
gcc -E -P -nostdinc -undef -D__DTS__ -x assembler-with-cpp \
    -I arch/arm/boot/dts -I include \
    arch/arm/boot/dts/omap3630-sgx530-capture.dts -o "$work/capture.pp.dts"
dtc -I dts -O dtb -i arch/arm/boot/dts -o "$work/capture.dtb" "$work/capture.pp.dts"
cat arch/arm/boot/zImage "$work/capture.dtb" > "$work/zImage-dtb"

# 6. uImage for bootm (load/entry match the board's c4test.img flow).
mkimage -A arm -O linux -T kernel -C none -a 0x80008000 -e 0x80008000 \
  -n sgx530-capture -d "$work/zImage-dtb" "$work/openhc-sgx530-capture.uImage"
echo "artifact: $work/openhc-sgx530-capture.uImage"
