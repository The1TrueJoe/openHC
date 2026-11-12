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
# busybox + the runtime libs the vendor stack links (all GPL/MIT/LGPL). gbm,
# wayland-server and libdrm_omap are linked for the display path and are inert
# at init; present only for the loader. libdrm-omap1 is the one to confirm in
# the archive — if absent, take libdrm_omap.so.1 from a TI SDK's libdrm build
# (freedesktop MIT, not an Imagination binary, so still fine in a GPL initramfs).
# Required set (static busybox needs no libs of its own).
pkgs="busybox-static:armhf libc6:armhf libdrm2:armhf libgbm1:armhf \
      libwayland-server0:armhf libudev1:armhf libffi6:armhf"
( cd "$work"; apt-get download $pkgs )
# libdrm_omap is linked but inert at init; best-effort (not in every archive).
( cd "$work"; apt-get download libdrm-omap1:armhf 2>/dev/null || \
  echo "note: libdrm-omap1 not in archive; take libdrm_omap.so.1 from a TI SDK libdrm (MIT) if the loader needs it" )
for d in "$work"/*.deb; do dpkg-deb -x "$d" "$work/root"; done

# --- stage into the initramfs ------------------------------------------------
cp -a "$work"/root/bin/busybox "$out/bin/busybox"
for a in sh mount umount ip ifconfig wget tar sleep mkdir cat echo \
         ping timeout nc dmesg grep head date; do
  ln -sf busybox "$out/bin/$a"
done
# ld-linux + the runtime .so (keep symlinks)
cp -a "$work"/root/lib/arm-linux-gnueabihf/. "$out/lib/" 2>/dev/null || true
cp -a "$work"/root/lib/ld-linux-armhf.so.3 "$out/lib/" 2>/dev/null || true
cp -a "$work"/root/usr/lib/arm-linux-gnueabihf/. "$out/usr/lib/" 2>/dev/null || true

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
echo "CAP: fetch TI user space from the Mac" > /dev/kmsg
wget -q -O /tmp/ti.tar "$MAC_HTTP/ti-userspace.tar" && tar -C /tmp -xf /tmp/ti.tar || echo "CAP: FETCH FAILED" > /dev/kmsg
export LD_LIBRARY_PATH=/tmp/ti/lib:/usr/lib:/lib
echo "CAP: start vendor init" > /dev/kmsg
timeout 90 /tmp/ti/bin/pvrsrvctl --start --no-module 2>&1 | while read l; do echo "CAP: \$l" > /dev/kmsg; done
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
