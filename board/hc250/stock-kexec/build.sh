#!/usr/bin/env bash
# Build ohc_kexec.ko for the stock HC-250 kernel (2.6.32-51.48) without its
# kbuild: headers from Control4's GPL drop, autoconf from the stock config,
# symbol CRCs from the stock modules (what modpost would have read from
# Module.symvers). Needs an ARM gcc and python3.
#
#   ./build.sh [out-dir]          (CC=arm-linux-gnueabihf-gcc to pick another gcc)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "${1:-$here/out}" && cd "${1:-$here/out}" && pwd)
CC=${CC:-arm-none-eabi-gcc}
LD=${LD:-${CC%gcc}ld}
NM=${NM:-${CC%gcc}nm}
URL=http://update.control4.com/open_src/2.9.0.525559-res/src/linux-omap-2.6.32+patches.tar.gz
MD5=0c34ca7239f6c7681078fbc431deb32e
tgz=$out/linux-omap-2.6.32+patches.tar.gz
L=$out/linux-omap

if [ ! -d "$L/include" ]; then
    [ -f "$tgz" ] || curl -fL -o "$tgz" "$URL"
    sum=$( (md5sum "$tgz" 2>/dev/null || md5 -r "$tgz") | cut -d' ' -f1)
    [ "$sum" = "$MD5" ] || { echo "md5 mismatch on $tgz" >&2; exit 1; }
    tar -xzf "$tgz" -C "$out" linux-omap-2.6.32.tar.bz2
    tar -xjf "$out/linux-omap-2.6.32.tar.bz2" -C "$out" \
        linux-omap/include linux-omap/arch/arm/include \
        linux-omap/arch/arm/mach-omap2/include linux-omap/arch/arm/plat-omap/include \
        linux-omap/kernel/bounds.c
    rm -f "$out/linux-omap-2.6.32.tar.bz2"
fi

# Generated headers a configured tree would have.
gen=$out/gen
mkdir -p "$gen/linux"
python3 -I - "$here/stock-kernel.config" "$gen/linux/autoconf.h" <<'EOF'
import re, sys
out = []
for l in open(sys.argv[1]):
    m = re.match(r'^(CONFIG_\w+)=(.*)$', l.strip())
    if m:
        k, v = m.groups()
        out.append(f"#define {k}_MODULE 1" if v == 'm' else f"#define {k} {1 if v == 'y' else v}")
open(sys.argv[2], 'w').write("\n".join(out) + "\n")
EOF
echo '#define UTS_RELEASE "2.6.32-51.48"' > "$gen/linux/utsrelease.h"
printf '#define LINUX_VERSION_CODE 132640\n#define KERNEL_VERSION(a,b,c) (((a) << 16) + ((b) << 8) + (c))\n' > "$gen/linux/version.h"
# 2.6.32 knows gcc up to 4: let any newer gcc use compiler-gcc4.h.
echo '#include <linux/compiler-gcc4.h>' > "$gen/linux/compiler-gcc$($CC -dumpversion | cut -d. -f1).h"
: > "$gen/linux/bounds.h"

CFLAGS=(-nostdinc -isystem "$($CC -print-file-name=include)"
    -I"$gen" -I"$L/include" -I"$L/arch/arm/include"
    -I"$L/arch/arm/mach-omap2/include" -I"$L/arch/arm/plat-omap/include"
    -include "$gen/linux/autoconf.h" -D__KERNEL__ -D__LINUX_ARM_ARCH__=7 -Uarm
    -std=gnu89 -fgnu89-inline -O2 -marm -mabi=aapcs-linux -mno-thumb-interwork
    -march=armv7-a -mtune=cortex-a8 -mfloat-abi=soft -mword-relocations -fno-pic
    -fno-common -fno-strict-aliasing -fno-delete-null-pointer-checks
    -fno-stack-protector -fno-unwind-tables -fno-asynchronous-unwind-tables
    -fno-omit-frame-pointer -mapcs-frame -fno-tree-loop-distribute-patterns
    -fno-ipa-sra -mno-unaligned-access -Wno-attributes)

# bounds.h the way kbuild makes it.
$CC "${CFLAGS[@]}" -S -o "$out/bounds.s" "$L/kernel/bounds.c"
sed -n 's/^->\([A-Z_]*\) [#$]*\([0-9]*\) .*/#define \1 \2/p' "$out/bounds.s" > "$gen/linux/bounds.h"
grep -q NR_PAGEFLAGS "$gen/linux/bounds.h"

mod=(-DMODULE -DKBUILD_MODNAME='"ohc_kexec"')
$CC "${CFLAGS[@]}" "${mod[@]}" -DKBUILD_BASENAME='"ohc_kexec"' -Wall -Wno-pointer-sign \
    -c "$here/ohc_kexec.c" -o "$out/ohc_kexec.o"
NM=$NM python3 -I "$here/mkmod.py" "$out/ohc_kexec.o" "$here/stock-crcs.txt" "$out/ohc_kexec.mod.c"
$CC "${CFLAGS[@]}" "${mod[@]}" -DKBUILD_BASENAME='"ohc_kexec.mod"' \
    -c "$out/ohc_kexec.mod.c" -o "$out/ohc_kexec.mod.o"
$LD -EL -r -o "$out/ohc_kexec.ko" "$out/ohc_kexec.o" "$out/ohc_kexec.mod.o"
cp "$here/ohc-kexec.sh" "$out/"
echo "built $out/ohc_kexec.ko"
