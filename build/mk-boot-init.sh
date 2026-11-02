#!/usr/bin/env bash
# Build the tiny boot-initramfs (boot-init.cpio.gz) that the EA autoscript loads.
#
# It carries only what board/ea/common/boot-init/init needs: busybox, its
# dynamic loader + libc, the /init itself, and the applet symlinks the script
# calls. A few hundred KB to ~2 MB, versus the ~15-20 MB full rootfs — so a
# normal boot unpacks almost nothing before switch_root frees it.
#
#   mk-boot-init.sh <TARGET_DIR> <OUT_cpio_gz> <init_script>
#
# TARGET_DIR is the assembled full rootfs (for busybox + libs); we copy out of
# it rather than rebuild, so the boot-init and the rootfs always share one
# busybox/libc ABI.
set -euo pipefail
TARGET="$1"; OUT="$2"; INIT="$3"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK"/{bin,sbin,usr/sbin,usr/share/udhcpc,dev,dev/pts,proc,sys,mnt,lib,etc,etc/dropbear,root,var}

cp -a "$TARGET/bin/busybox" "$WORK/bin/busybox"
install -m755 "$INIT" "$WORK/init"

# Dynamic loader (the ELF interpreter busybox names) + libc + the handful of
# libraries busybox commonly pulls in. The loader is NOT optional — without it
# the kernel cannot exec busybox and /init never runs (the box hangs). Copy
# whatever of these exists in the target; missing optional ones are harmless.
# We glob the whole set rather than parse the interpreter path, because that
# parse was fragile in-container and silently dropped ld-linux.so.2.
mkdir -p "$WORK/lib"
for pat in 'ld-linux*.so*' 'ld-musl*.so*' 'ld-*.so*' \
           'libc.so*' 'libm.so*' 'libresolv.so*' 'libcrypt.so*' \
           'libpthread.so*' 'libdl.so*' 'librt.so*'; do
    for f in "$TARGET/lib/"$pat; do
        [ -e "$f" ] && cp -aL "$f" "$WORK/lib/" 2>/dev/null || true
    done
done
# dropbear's extra shared deps (on top of the libc set above). libcrypt is the
# one that bites: dropbear links crypt() for shadow auth, and Buildroot's
# libxcrypt installs libcrypt.so.2 under /usr/lib — which the /lib-only glob
# above misses, so dropbear would fail to load and the RAM installer would come
# up with NO ssh (strandable only over serial). Confirmed on an EA3: dropbear
# NEEDED libcrypt.so.2, absent, boot-init never reachable. zlib + libutil are
# dropbear's other common deps; harmless when unused.
for pat in 'libcrypt.so*' 'libz.so*' 'libutil.so*' 'libnss_files.so*' 'libnss_compat.so*'; do
    for f in "$TARGET/lib/"$pat "$TARGET/usr/lib/"$pat; do
        [ -e "$f" ] && cp -aL "$f" "$WORK/lib/" 2>/dev/null || true
    done
done
# Sanity: refuse to ship a boot-init busybox cannot start.
if ! ls "$WORK"/lib/ld-*.so* >/dev/null 2>&1; then
    echo "mk-boot-init: ERROR no dynamic loader (ld-*.so) found in $TARGET/lib" >&2
    exit 1
fi
# The x86_64 busybox's PT_INTERP is /lib64/ld-linux-x86-64.so.2, but the loader
# was copied into /lib. Without /lib64 the kernel cannot resolve the interpreter
# and `exec /init` fails with ENOENT -> "Kernel panic - No working init found".
# A /lib64 -> lib symlink makes the interp path resolve. PROVEN failure on an EA3
# (x86_64 userspace): every boot-init panicked here until this symlink existed.
ln -sf lib "$WORK/lib64"

# Applet symlinks the /init calls (busybox resolves the rest by argv[0]). The
# second group is the no-serial network-install path (see board/.../boot-init/init
# net_serve): bring up eth0, DHCP, and a shell for the flasher's SSH session.
for a in sh mount umount mkdir dd gunzip gzip sync switch_root cat ls \
         ifconfig udhcpc ip route sleep hostname echo sed cut \
         rm tar unxz xz; do
    ln -sf /bin/busybox "$WORK/bin/$a"
done
ln -sf /bin/busybox "$WORK/sbin/switch_root"
ln -sf /bin/busybox "$WORK/sbin/reboot"
for a in ifconfig udhcpc ip route; do ln -sf /bin/busybox "$WORK/sbin/$a"; done

# ---- one-shot stock restore (restore stock on durable leaves a flag on p1;
# board/.../boot-init/init runs the finalize here, pre-pivot, where p1 is free).
# restore is a static(-pie) musl binary, so it needs no libraries of its own;
# the reimage uses only the busybox tar/unxz applets added above (p1 keeps its
# ext4, so no mke2fs/e2fsprogs). If it is absent the flag path simply no-ops and a
# normal openHC boot proceeds. Copied from the assembled rootfs to share one ABI.
for rb in "$TARGET/usr/sbin/restore" "$TARGET/opt/ohc/bin/restore" \
          "$TARGET/usr/bin/restore" "$TARGET/sbin/restore"; do
    [ -x "$rb" ] && { cp -aL "$rb" "$WORK/usr/sbin/restore"; break; }
done
if [ -x "$WORK/usr/sbin/restore" ]; then
    echo "mk-boot-init: restore included (one-shot stock restore available)"
else
    echo "mk-boot-init: WARNING no restore in $TARGET — one-shot stock restore unavailable" >&2
fi

# ---- no-serial network install: dropbear + an account the flasher can log into
# dropbear is the SSH server the flasher streams the rootfs to p1 over (reusing
# its existing SSH stage2). Copied from the full rootfs so it shares one ABI. If
# the image has no dropbear the network path is simply unavailable (net_serve
# guards on the binary), and the on-device staged-image install still works.
for db in "$TARGET/usr/sbin/dropbear" "$TARGET/sbin/dropbear" "$TARGET/usr/bin/dropbear"; do
    [ -x "$db" ] && { cp -aL "$db" "$WORK/usr/sbin/dropbear"; break; }
done
if [ -x "$WORK/usr/sbin/dropbear" ]; then
    # root's login = the openHC default password, which is the first credential
    # ohc-flash's first_working_login tries (crates/transport/src/ssh.rs). Hash
    # generated at build when openssl is present, else a fixed precomputed $6$.
    HASH="$(openssl passwd -6 -salt ohcboot1 openhc 2>/dev/null || true)"
    [ -n "$HASH" ] || HASH='$6$ohcboot1$IzoDlPnv29L3Y9der0/4ha.GpfcGgMnTri7bOa.XlvYU3HWJCb3OMMtRbTBHmUAXL7AFwSe/9hV064yjH8ml81'
    printf 'root:x:0:0:root:/root:/bin/sh\n' > "$WORK/etc/passwd"
    printf 'root:%s:19000:0:99999:7:::\n' "$HASH" > "$WORK/etc/shadow"
    printf 'root:x:0:\n' > "$WORK/etc/group"
    chmod 600 "$WORK/etc/shadow"
    # busybox udhcpc needs this script to actually apply the lease it gets.
    cat > "$WORK/usr/share/udhcpc/default.script" <<'UDHCP'
#!/bin/sh
case "$1" in
  bound|renew)
    /sbin/ifconfig "$interface" "$ip" netmask "${subnet:-255.255.255.0}"
    [ -n "$router" ] && /sbin/route add default gw "$router" dev "$interface" 2>/dev/null
    : > /etc/resolv.conf
    for d in $dns; do echo "nameserver $d" >> /etc/resolv.conf; done
    ;;
  deconfig) /sbin/ifconfig "$interface" 0.0.0.0 ;;
esac
exit 0
UDHCP
    chmod 755 "$WORK/usr/share/udhcpc/default.script"
    echo "mk-boot-init: dropbear + network install path included"
else
    echo "mk-boot-init: WARNING no dropbear in $TARGET — no-serial network install unavailable" >&2
fi

( cd "$WORK" && find . | cpio -o -H newc 2>/dev/null | gzip -9 ) > "$OUT"
echo "mk-boot-init: $OUT ($(wc -c < "$OUT") bytes)"
