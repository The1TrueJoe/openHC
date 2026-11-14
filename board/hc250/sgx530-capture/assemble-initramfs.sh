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
# The REAL libs the vendor stack actually calls at init: glibc, the GCC runtime
# (libgcc_s, pulled in transitively), libdrm, libudev, libffi, libwayland-server.
pkgs="busybox-static:armhf libc6:armhf libgcc1:armhf libdrm2:armhf \
      libwayland-server0:armhf libudev1:armhf libffi6:armhf \
      dropbear-bin:armhf libtomcrypt0:armhf libtommath1:armhf zlib1g:armhf libgmp10:armhf"
( cd "$work"; apt-get download $pkgs )
for d in "$work"/*.deb; do dpkg-deb -x "$d" "$work/root"; done

# --- stage into the initramfs ------------------------------------------------
cp -a "$work"/root/bin/busybox "$out/bin/busybox"
for a in sh mount umount ip ifconfig wget tar sleep mkdir cat echo ls tr \
         ping timeout nc dmesg grep head date basename insmod modprobe lsmod \
         chown chmod cp rm mv md5sum hexdump od sed awk ps kill uptime free df \
         dd sync printf; do
  ln -sf busybox "$out/bin/$a"
done
# Runtime libs: copy /lib and /usr/lib PRESERVING the Debian multiarch layout,
# so /lib/ld-linux-armhf.so.3 (the ELF interpreter, a symlink into
# arm-linux-gnueabihf/) resolves. Flattening it breaks exec with ENOENT.
cp -a "$work"/root/lib/. "$out/lib/" 2>/dev/null || true
[ -d "$work"/root/usr/lib ] && cp -a "$work"/root/usr/lib/. "$out/usr/lib/" 2>/dev/null || true
# dropbear (stretch 2016.74: ECDSA host key, ECDSA client key) for an interactive
# session on the capture boot; key login only, the Mac's capture key.
cp -a "$work"/root/usr/sbin/dropbear "$out/sbin/dropbear"
cp -a "$work"/root/usr/bin/dropbearkey "$out/bin/dropbearkey"
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out/root/.ssh" "$out/etc/dropbear" "$out/dev/pts"
cp "$here/capture_authorized_key.pub" "$out/root/.ssh/authorized_keys"
echo 'root:x:0:0:root:/root:/bin/sh' > "$out/etc/passwd"
echo 'root:x:0:' > "$out/etc/group"
echo '/bin/sh' > "$out/etc/shells"
# The render test (built in a stretch container by build.sh).
[ -n "${RENDER_TEST:-}" ] && cp "$RENDER_TEST" "$out/render-test" && chmod +x "$out/render-test"

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
for need in ld-linux-armhf.so.3 libc.so.6 libgcc_s.so.1 libdrm.so.2 libudev.so.1 \
            libffi.so.6 libwayland-server.so.0 libgbm.so.2 libdrm_omap.so.1 \
            libdl.so.2 libpthread.so.0 librt.so.1; do
  find "$out/lib" "$out/usr/lib" -name "$need" 2>/dev/null | grep -q . \
    || { echo "ERROR: required lib $need missing from the initramfs"; \
         find "$out/lib" "$out/usr/lib" -name '*.so*' | sort; exit 1; }
done
# Transitive closure: every NEEDED soname of every staged .so must itself be
# present (catches deps-of-deps like libgcc_s that a direct-NEEDED walk misses).
rd="${CROSS:-arm-linux-gnueabihf-}readelf"
miss=0
for so in $(find "$out/lib" "$out/usr/lib" -name '*.so*' -type f); do
  for n in $("$rd" -d "$so" 2>/dev/null | sed -n 's/.*NEEDED.*\[\(.*\)\].*/\1/p'); do
    find "$out/lib" "$out/usr/lib" -name "$n" 2>/dev/null | grep -q . \
      || { echo "ERROR: transitive dep $n (needed by $(basename "$so")) missing"; miss=1; }
  done
done
[ "$miss" -eq 0 ] || { echo "transitive lib closure incomplete"; exit 1; }
echo "initramfs libs OK (reals + stubs for gbm/drm_omap; transitive closure complete)"

# --- init ---------------------------------------------------------------------
# No serial: logs and snapshots go to the Mac's sink (NETCONSOLE_TARGET host:port)
# over nc, one connection per payload, each starting with a "==NAME==" line. The
# Mac runs a loop of `nc -l <port>`. After the scripted run, dropbear stays up
# (key login only) for follow-up work on the same boot.
: "${NETCONSOLE_TARGET:?set NETCONSOLE_TARGET=<mac-ip>:<port>}"
sink_host=${NETCONSOLE_TARGET%:*}; sink_port=${NETCONSOLE_TARGET##*:}
cat > "$out/init" <<INIT
#!/bin/sh
mount -t proc none /proc; mount -t sysfs none /sys
mount -t devtmpfs none /dev 2>/dev/null || true
mkdir -p /dev/pts; mount -t devpts devpts /dev/pts
mount -t tmpfs none /tmp
mount -t debugfs none /sys/kernel/debug
chown -R 0:0 /root; chmod 700 /root /root/.ssh; chmod 600 /root/.ssh/authorized_keys
ifconfig lo 127.0.0.1 up
ifconfig eth0 10.0.0.123 up 2>/dev/null || ip addr add 10.0.0.123/24 dev eth0
i=0; while [ \$i -lt 15 ]; do ping -c1 -W1 $sink_host >/dev/null 2>&1 && break; i=\$((i+1)); sleep 1; done
ship() { name=\$1; shift; { echo "==\$name=="; "\$@"; } | nc $sink_host $sink_port; sleep 1; }
# Host key: generated now, its public half shipped before dropbear listens.
dropbearkey -t ecdsa -s 256 -f /etc/dropbear/dropbear_ecdsa_host_key >/dev/null 2>&1
ship HOSTKEY dropbearkey -y -f /etc/dropbear/dropbear_ecdsa_host_key
dropbear -r /etc/dropbear/dropbear_ecdsa_host_key -s -p 22
echo "CAP: insmod pvrsrvkm" > /dev/kmsg
insmod /pvrsrvkm.ko 2>&1 | while read l; do echo "CAP: insmod: \$l" > /dev/kmsg; done
wget -q -O /tmp/ti.tar "$MAC_HTTP/ti-userspace.tar" && tar -C /tmp -xf /tmp/ti.tar || echo "CAP: FETCH FAILED" > /dev/kmsg
cp /tmp/ti/powervr.ini /etc/powervr.ini 2>/dev/null
export LD_LIBRARY_PATH=/tmp/ti/lib:/usr/lib/arm-linux-gnueabihf:/lib/arm-linux-gnueabihf:/usr/lib:/lib
echo "CAP: dri=\$(ls /dev/dri 2>&1 | tr '\n' ' ') drm=\$(cat /sys/class/drm/*/device/uevent 2>/dev/null | grep DRIVER | tr '\n' ' ')" > /dev/kmsg
echo "CAP: start vendor init" > /dev/kmsg
timeout -t 90 /tmp/ti/bin/pvrsrvctl --start --no-module > /tmp/pvrsrvctl.log 2>&1
echo "CAP: eglinfo" > /dev/kmsg
timeout -t 30 /tmp/ti/bin/eglinfo > /tmp/eglinfo.log 2>&1
echo "CAP: render-test" > /dev/kmsg
timeout -t 60 /render-test > /tmp/rt.log 2>&1
echo "CAP: render-test rc=\$?" > /dev/kmsg
echo "CAP: gles2test1" > /dev/kmsg
( cd /tmp/ti/bin && timeout -t 15 ./gles2test1 > /tmp/gles2test1.log 2>&1 )
echo "CAP: gles2test1 rc=\$?" > /dev/kmsg
ship LOGS sh -c 'for f in /tmp/pvrsrvctl.log /tmp/eglinfo.log /tmp/rt.log /tmp/gles2test1.log; do echo "--- \$f"; cat \$f; done; echo "--- dmesg"; dmesg'
[ -f /tmp/rt.rgba ] && ship RTRGBA cat /tmp/rt.rgba
for n in 0 1 2; do
  [ -e /sys/kernel/debug/sgxsnap\$n ] && ship SNAP\$n cat /sys/kernel/debug/sgxsnap\$n
done
ship DONE echo "capture done; dropbear on :22"
exec sh
INIT
chmod +x "$out/init"

# --- pack ---------------------------------------------------------------------
( cd "$out" && find . | cpio -o -H newc | gzip -9 ) > "$work/initramfs.cpio.gz"
cp "$work/initramfs.cpio.gz" "$out/.cpio.gz"
echo "GPL-only initramfs staged at $out (cpio at $out/.cpio.gz)"
