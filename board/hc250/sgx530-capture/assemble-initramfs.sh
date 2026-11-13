#!/usr/bin/env bash
# Assemble the GPL-only armhf init-only initramfs for the SGX530 vendor capture.
#
# Carries NO TI/Imagination binaries. It stages busybox and the GPL/distro armhf
# runtime libs the vendor user space links, plus an init that, at run time,
# fetches the closed TI user space from the Mac over http into a tmpfs and runs
# it. The TI files never enter this initramfs, the CI artifact, or any repo.
#
# Runs on ubuntu-24.04 CI. The old TI binaries are glibc-2.4/2016 vintage, so
# the runtime libs are pinned from Debian stretch armhf (archive.debian.org).
set -euo pipefail
out=${1:?initramfs dir}
: "${MAC_HTTP:?set MAC_HTTP=http://<mac-ip>:<port> (serves the TI tarball at run time)}"
work=$(mktemp -d)
rm -rf "$out"; mkdir -p "$out"/{bin,sbin,lib,dev,proc,sys,etc,tmp,usr/lib,usr/bin}

# --- fetch armhf packages from Debian stretch (archived) ---------------------
# CI runs as non-root: sudo the privileged apt/dpkg steps. Pin the runner's own
# (Ubuntu) sources to amd64 first, so adding the armhf arch does not make apt
# try to fetch armhf from Ubuntu's amd64-only mirrors (which 404 the update).
for f in /etc/apt/sources.list.d/ubuntu.sources; do
  [ -f "$f" ] && sudo sed -i '/^Signed-By:/a Architectures: amd64' "$f"
done
sudo dpkg --add-architecture armhf
echo 'deb [trusted=yes arch=armhf] http://archive.debian.org/debian stretch main' \
  | sudo tee /etc/apt/sources.list.d/stretch-armhf.list >/dev/null
sudo apt-get -o Acquire::Check-Valid-Until=false update
# The REAL libs the vendor stack actually calls at init: glibc, libdrm, libudev,
# libffi, libwayland-server. These are present and version-compatible in stretch.
pkgs="busybox-static:armhf libc6:armhf libdrm2:armhf \
      libwayland-server0:armhf libudev1:armhf libffi6:armhf"
( cd "$work"; apt-get download $pkgs )
for d in "$work"/*.deb; do dpkg-deb -x "$d" "$work/root"; done

# --- stage into the initramfs ------------------------------------------------
cp -a "$work"/root/bin/busybox "$out/bin/busybox"
for a in sh mount umount ip ifconfig wget tar sleep mkdir cat echo ls tr \
         ping timeout nc dmesg grep head date basename insmod modprobe lsmod; do
  ln -sf busybox "$out/bin/$a"
done
# Runtime libs: copy /lib and /usr/lib PRESERVING the Debian multiarch layout,
# so /lib/ld-linux-armhf.so.3 (the ELF interpreter, a symlink into
# arm-linux-gnueabihf/) resolves. Flattening it breaks exec with ENOENT.
cp -a "$work"/root/lib/. "$out/lib/" 2>/dev/null || true
[ -d "$work"/root/usr/lib ] && cp -a "$work"/root/usr/lib/. "$out/usr/lib/" 2>/dev/null || true
# Fail the build if the ARM dynamic linker is missing (run 2: pvrsrvctl ENOENT).
[ -e "$out/lib/ld-linux-armhf.so.3" ] || { echo "ERROR: ld-linux-armhf.so.3 missing"; ls -la "$out/lib"; exit 1; }

# Stub the display-path sonames the 2019-era TI binaries link but do NOT call at
# `pvrsrvctl --start`: libgbm.so.2 (stretch ships only .so.1) and libdrm_omap.so.1
# (dropped from Debian). A minimal .so with the right SONAME satisfies the loader
# (NEEDED is resolved by soname; the unused symbols bind lazily and are never
# called at init). This is a GPL/trivial stub, no TI/Imagination code.
cc="${CROSS:-arm-linux-gnueabihf-}gcc"
mkdir -p "$out/usr/lib/arm-linux-gnueabihf"
for soname in libgbm.so.2 libdrm_omap.so.1; do
  printf 'int __stub(void){return 0;}\n' | \
    "$cc" -shared -nostdlib -fPIC -Wl,-soname,"$soname" \
      -o "$out/usr/lib/arm-linux-gnueabihf/$soname" -x c - \
    || { echo "ERROR: could not build stub $soname"; exit 1; }
done

# Fail the build NOW (not on the board) if any lib the TI user space needs is
# absent — real or stub. Search the whole initramfs lib tree by soname.
for need in ld-linux-armhf.so.3 libc.so.6 libdrm.so.2 libudev.so.1 libffi.so.6 \
            libwayland-server.so.0 libgbm.so.2 libdrm_omap.so.1; do
  find "$out/lib" "$out/usr/lib" -name "$need" 2>/dev/null | grep -q . \
    || { echo "ERROR: required lib $need missing from the initramfs"; \
         find "$out/lib" "$out/usr/lib" -name '*.so*' | sort; exit 1; }
done
echo "initramfs libs OK (reals + stubs for gbm/drm_omap)"

# --- init ---------------------------------------------------------------------
# The board has no serial and this initramfs runs no sshd, so the log must come
# back over the network: the init time-bounds pvrsrvctl (a GPU hang can't block
# it) and ships the whole dmesg to the Mac's sink (NETCONSOLE_TARGET host:port)
# over nc. The sink is the Mac's `nc -l <port> > cap.log`.
: "${NETCONSOLE_TARGET:?set NETCONSOLE_TARGET=<mac-ip>:<port>}"
sink_host=${NETCONSOLE_TARGET%:*}; sink_port=${NETCONSOLE_TARGET##*:}
cat > "$out/init" <<INIT
#!/bin/sh
mount -t proc none /proc; mount -t sysfs none /sys
mount -t devtmpfs none /dev 2>/dev/null || true
mount -t tmpfs none /tmp
ifconfig eth0 10.0.0.123 up 2>/dev/null || ip addr add 10.0.0.123/24 dev eth0
# wait for the link to the Mac (up to ~15 s)
i=0; while [ \$i -lt 15 ]; do ping -c1 -W1 $sink_host >/dev/null 2>&1 && break; i=\$((i+1)); sleep 1; done
# Load the openpvrsgx services module: it registers the platform driver that
# binds 50000000.gpu and creates /dev/pvrsrvkm for the user space.
echo "CAP: insmod pvrsrvkm" > /dev/kmsg
insmod /pvrsrvkm.ko 2>&1 | while read l; do echo "CAP: insmod: \$l" > /dev/kmsg; done
echo "CAP: fetch TI user space from the Mac" > /dev/kmsg
wget -q -O /tmp/ti.tar "$MAC_HTTP/ti-userspace.tar" && tar -C /tmp -xf /tmp/ti.tar || echo "CAP: FETCH FAILED" > /dev/kmsg
export LD_LIBRARY_PATH=/tmp/ti/lib:/usr/lib/arm-linux-gnueabihf:/lib/arm-linux-gnueabihf:/usr/lib:/lib
# Bind diagnostics: run 1 showed the GPU ti-sysc module never probed, so gpu@0
# was never created and openpvrsgx (binds "ti,omap3-sgx530-125") had nothing to
# attach to. Surface the platform device/driver state, and if the device exists
# but is unbound, try a manual bind — all in this one slot.
echo "CAP: dev=\$(ls /dev 2>/dev/null | grep -iE 'pvr|drm|dri' | tr '\n' ' ')" > /dev/kmsg
echo "CAP: platdev=\$(ls /sys/bus/platform/devices/ 2>/dev/null | grep -iE '5000|gpu|sgx' | tr '\n' ' ')" > /dev/kmsg
echo "CAP: platdrv=\$(ls /sys/bus/platform/drivers/ 2>/dev/null | grep -iE 'sgx|pvr|omap3-sgx' | tr '\n' ' ')" > /dev/kmsg
drv=\$(ls /sys/bus/platform/drivers/ 2>/dev/null | grep -iE 'sgx|pvr|omap3-sgx' | head -1)
for dev in \$(ls /sys/bus/platform/devices/ 2>/dev/null | grep -iE '5000.*(gpu|sgx)|sgx530'); do
  if [ -n "\$drv" ] && [ ! -e /sys/bus/platform/devices/\$dev/driver ]; then
    echo "CAP: manual bind \$dev -> \$drv" > /dev/kmsg
    echo "\$dev" > /sys/bus/platform/drivers/\$drv/bind 2>&1 | while read l; do echo "CAP: bind: \$l" > /dev/kmsg; done
  fi
done
echo "CAP: pvr-dmesg-lines=\$(dmesg | grep -ic pvr)  ti=\$(ls -l /tmp/ti/bin/pvrsrvctl 2>&1 | head -c 80)" > /dev/kmsg
echo "CAP: start vendor init" > /dev/kmsg
# busybox (stretch) timeout uses -t SECONDS; a bare duration is read as the command.
timeout -t 90 /tmp/ti/bin/pvrsrvctl --start --no-module 2>&1 | while read l; do echo "CAP: \$l" > /dev/kmsg; done
echo "CAP: vendor init returned; shipping dmesg to the Mac" > /dev/kmsg
# Primary: ship the whole kernel log to the Mac (the CAP-* lines are in it).
{ echo "===CAP-DMESG-BEGIN==="; dmesg; echo "===CAP-DMESG-END==="; } | nc $sink_host $sink_port
sleep 3
# Retry just the CAP lines, in case the first shipment raced boot.
{ echo "===CAP-ONLY-BEGIN==="; dmesg | grep -E 'CAP|SGX|PVR|BIF'; echo "===CAP-ONLY-END==="; } | nc $sink_host $sink_port
echo "CAP: shipped; board can be rebooted" > /dev/kmsg
sleep 60
exec sh
INIT
chmod +x "$out/init"

# --- pack ---------------------------------------------------------------------
( cd "$out" && find . | cpio -o -H newc | gzip -9 ) > "$work/initramfs.cpio.gz"
cp "$work/initramfs.cpio.gz" "$out/.cpio.gz"
echo "GPL-only initramfs staged at $out (cpio at $out/.cpio.gz)"
