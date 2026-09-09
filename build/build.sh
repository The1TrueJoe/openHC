#!/usr/bin/env bash
# In-container Buildroot builder. Invoked by build/Dockerfile during
# `docker build` (see the Makefile `image` target). Runs against the Buildroot
# tree at $BUILDROOT_DIR, writing to cache-mounted dl/ and output/ so nothing
# touches the macOS bind mount (which crashed Docker Desktop's virtiofs under
# Buildroot's heavy package I/O).
#
#   ./build.sh            # configure + build the image (default)
#   ./build.sh <target>   # any Buildroot make target
#
# BOARD selects which defconfig to build (board/<board>/<board>_defconfig).
# Job count auto-caps by RAM (~1.5GB/job, min with CPU count). Override BR2_JLEVEL.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
# Keep this default in sync with the Makefile's `BOARD ?=`. Note BOARD is an
# ENV var — $1 is passed through as a Buildroot make target, so
# `./build.sh ea3-v2` does NOT select a board, it asks make to build a target
# called "ea3-v2". Use `make image BOARD=ea3-v2` or `BOARD=ea3-v2 ./build.sh`.
BOARD="${BOARD:-ea3-v2}"
BUILDROOT_DIR="${BUILDROOT_DIR:-/opt/buildroot}"
# Per-board output tree: the two boards build different kernels, so sharing one
# output dir would force a full rebuild on every board switch.
OUT="${BR2_OUTPUT_DIR:-$REPO/output/build/$BOARD}"
DL="${BR2_DL_DIR:-$REPO/dl}"

# The shared "family" base is picked by board name: ea* -> ea-common (Intel
# CE5300, x86). A board with no family base builds from its own defconfig alone
# — that is ca1 (i.MX6SL), ioxv1 (DM355) and hc800 (Atom D525) today,
# deliberately: a family with one member in it would be indirection with
# nothing behind it. Add a ca-common when a second CA-series board lands.
#
# hc800 will never get a family base. It is a PC, not a Control4 SoC board, and
# the only other member of its IO-MCU family (hc250) is ARMv7 and shares
# nothing else with it.
case "$BOARD" in
  ea*)  COMMON_CFG="$REPO/board/ea-common/ea-common_defconfig" ;;
  *)    COMMON_CFG="" ;;
esac
BOARD_CFG="$REPO/board/$BOARD/${BOARD}_defconfig"
[ -f "$BOARD_CFG" ] || {
    echo "no such board '$BOARD' — have:" >&2
    for d in "$REPO"/board/*/; do
        b=$(basename "$d")
        [ -f "$d/${b}_defconfig" ] && echo "  $b" >&2
    done
    exit 1
}
echo ">> board=$BOARD"

# nproc and /proc/meminfo are Linux; the real builds run in the container. But
# OHC_DEFCONFIG_ONLY below is useful from a Mac, and it would be silly for a
# dry run that compiles nothing to fail on a job-count probe. Fall back rather
# than refuse.
cpu=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)
memkb=$(awk '/^MemTotal:/{print $2}' /proc/meminfo 2>/dev/null \
        || { hw=$(sysctl -n hw.memsize 2>/dev/null) && echo $((hw / 1024)); } \
        || echo 4194304)
# 2.5 GB/job, not 1.5. gcc's largest translation units (insn-emit.o and friends
# in host-gcc-initial) peak well above 1.5 GB each, so the old figure let two of
# them run side by side in a 3.8 GB VM and the OOM killer took one out:
#
#     g++: fatal error: Killed signal terminated program cc1plus
#     ResourceExhausted: ... cannot allocate memory
#
# It was marginal rather than always-fatal, which is worse — the same tree built
# fine one run and died the next. Being pessimistic here costs wall-clock and
# buys reproducibility.
#
# Measured 2026-08-28 on a 12-core/16 GB machine, and the first measurement was
# WRONG in a way worth recording. Sampling 6 jobs mid-WebKit gave ~6.7 GB total,
# ~1.1 GB/job, and that looked like proof the 2.5 GB figure was set by
# host-gcc-initial alone. Raised to 10 on that basis. Some hours later:
#
#     MemAvailable 1.5 GB, 400 MB swapped out, 10 cc1plus holding 12.3 GB,
#     largest single TU 1788 MB
#
# WebCore's heaviest translation units really do approach 1.8 GB; the earlier
# sample had simply landed in a light stretch. An average taken from one phase
# of a build that is not uniform is not a bound.
#
# So the cap is closer to right than it looked. Raising BR2_JLEVEL still helps
# -- 10 ran ~1.6-2x faster than 6 before it started swapping -- but it wants
# headroom for ~1.8 GB peaks, not the average. 8 is the compromise that fits
# 16 GB. Ninja resumes cleanly if interrupted: an edge missing from .ninja_log
# is redone, so a partial .o is never mistaken for a finished one.
memjobs=$(( memkb / 2500000 )); [ "$memjobs" -lt 1 ] && memjobs=1
# An EXPLICIT BR2_JLEVEL wins, including over the RAM cap. It used to be
# clamped like the default, which made the "Override BR2_JLEVEL" above a lie:
# asking for 10 on a 16 GB box silently got you 6, with the log cheerfully
# printing the number you did not ask for. The cap exists to pick a safe
# DEFAULT, not to overrule someone who has looked at their machine.
if [ -n "${BR2_JLEVEL:-}" ]; then
    JOBS="$BR2_JLEVEL"
    if [ "$memjobs" -lt "$JOBS" ]; then
        echo ">> BR2_JLEVEL=$JOBS explicitly, above the mem-safe $memjobs"
        echo ">>   safe once the toolchain is built (~1.1GB/job for WebKit);"
        echo ">>   host-gcc-initial is what needs 2.5GB, so a tree that has to"
        echo ">>   rebuild it can still OOM at this level."
    fi
else
    JOBS="$cpu"
    [ "$memjobs" -lt "$JOBS" ] && JOBS="$memjobs"
fi
echo ">> BR2_JLEVEL=$JOBS (cpu=$cpu, mem-safe=$memjobs, MemTotal=$((memkb/1024))MB)"
if [ "$memjobs" -lt 2 ]; then
    echo ">> NOTE: only ${JOBS} job — this VM has $((memkb/1024))MB."
    echo ">>       Raising Docker Desktop's memory to 8GB roughly halves the build."
fi

# Low-memory warning. Capping the job count is NOT sufficient protection: a
# single cc1plus building gcc can exhaust a small VM on its own, and Buildroot
# under memory pressure fails in ways that look nothing like memory pressure.
# On a 4 GB Docker Desktop VM this cost a long debugging detour — the build died
# with
#
#     ./config.status: line 1079: syntax error near unexpected token `fi'
#     make[1]: *** [Makefile:13201: configure-target-libgcc] Error 1
#
# which reads as a broken toolchain configuration but was really the OOM killer
# truncating a generated shell script. The same tree built cleanly on the retry.
# So: say it up front, because the error you get later will not.
memmb=$(( memkb / 1024 ))
if [ "$memmb" -lt 6000 ]; then
    echo ">> WARNING: only ${memmb} MB RAM visible to this build." >&2
    echo ">>   Buildroot wants ~6 GB+ (8 GB comfortable). Below that it can be" >&2
    echo ">>   OOM-killed mid-package, and the failure often surfaces as a bogus" >&2
    echo ">>   'syntax error' in a generated configure/config.status rather than" >&2
    echo ">>   as an out-of-memory message. If you hit one of those, check" >&2
    echo ">>   'dmesg | grep -i oom' in the VM before believing the error." >&2
    echo ">>   On Docker Desktop: Settings -> Resources -> Memory." >&2
fi

# Composing a defconfig needs none of Buildroot, so OHC_DEFCONFIG_ONLY skips
# this — that is what makes the dry run usable outside the build container.
[ -n "${OHC_DEFCONFIG_ONLY:-}" ] || \
    [ -f "$BUILDROOT_DIR/Makefile" ] || { echo "no Buildroot at $BUILDROOT_DIR" >&2; exit 1; }
mkdir -p "$OUT" "$DL"

# The effective defconfig is the family base + the board's deltas, concatenated.
# Buildroot/kconfig takes the LAST assignment of a symbol, so the board file
# extends or overrides the common one just by appearing after it. Buildroot has
# no include mechanism for defconfigs, hence the concatenation.
EFFECTIVE="$OUT/openhc-${BOARD}.defconfig"
mkdir -p "$OUT"
# Three layers now, each overriding the previous (kconfig takes the LAST
# assignment): common (every board) -> family (ea-common, EA boards only) ->
# board. Anything true fleet-wide lives in common/common_defconfig so the board
# files stay short lists of genuine differences.
ALL_CFG="$REPO/board/common/common_defconfig"

# Feature sets. A feature is a shared capability a board either has or does not,
# named in an ohc.features file (one word per line or space separated, '#'
# comments ignored). This is what keeps board files short: ea3-v1 and ea1-v2
# differ from their siblings by a word here, not by a copied block of
# wpa_supplicant/ext4 settings.
#
# The LIST layers the same way the defconfigs do — family first, then board —
# so a capability every EA controller has is stated once in
# board/ea-common/ohc.features rather than five times. Duplicates are harmless
# and removed.
FEATURES=""
for _ff in ${COMMON_CFG:+"$(dirname "$COMMON_CFG")/ohc.features"} "$REPO/board/$BOARD/ohc.features"; do
    [ -f "$_ff" ] || continue
    FEATURES="$FEATURES $(sed 's/#.*//' "$_ff" | tr '\n' ' ')"
done
FEATURES=$(printf '%s\n' $FEATURES | awk '!seen[$0]++' | tr '\n' ' ')

# A feature is ONE DIRECTORY holding both of its halves:
#
#     features/<name>/defconfig       the userspace half (Buildroot packages)
#     features/<name>/linux.fragment  the kernel half
#
# They used to be features/<name>.defconfig and linux/<name>.fragment, paired
# only by filename across two directories. That split is how the `switch`
# feature once shipped its userspace half with no kernel half — the board
# booted, and the driver simply was not there. Together, a feature cannot
# half-exist.
#
# Features are SEARCHED across the same three scopes the defconfigs layer
# through — common, family, board — most general first. So `splash` lives once
# in board/common/features/ and is selected by an HC-800 and an EA3 alike, while
# a family or a board can define one of its own without touching common.
feature_dir() {
    for _d in "$REPO/board/common/features/$1" \
              ${COMMON_CFG:+"$(dirname "$COMMON_CFG")/features/$1"} \
              "$REPO/board/$BOARD/features/$1"; do
        [ -d "$_d" ] && { printf '%s\n' "$_d"; return 0; }
    done
    return 1
}

# Kernel fragments that accompany the selected features, as BR2_EXTERNAL-relative
# paths (Buildroot expands $(BR2_EXTERNAL_OPENHC_PATH) itself).
FEAT_FRAGMENTS=""
for f in $FEATURES; do
    d=$(feature_dir "$f") || continue
    [ -f "$d/linux.fragment" ] || continue
    FEAT_FRAGMENTS="$FEAT_FRAGMENTS \$(BR2_EXTERNAL_OPENHC_PATH)/${d#"$REPO/board/"}/linux.fragment"
done

{
    echo "# GENERATED by build/build.sh — do not edit."
    echo "# common + ${COMMON_CFG:+${COMMON_CFG##*/board/} + }${FEATURES:+features[$FEATURES] + }$BOARD/${BOARD}_defconfig"
    echo
    if [ -f "$ALL_CFG" ]; then
        cat "$ALL_CFG"
        echo
    fi
    if [ -n "$COMMON_CFG" ]; then
        cat "$COMMON_CFG"
        echo
    fi
    for f in $FEATURES; do
        d=$(feature_dir "$f") || {
            echo "build.sh: unknown feature '$f' — looked in common, ${COMMON_CFG:+$(basename "$(dirname "$COMMON_CFG")"), }$BOARD" >&2
            exit 1
        }
        if [ -f "$d/defconfig" ]; then
            echo "# --- feature: $f (${d#"$REPO/board/"}) ---"
            cat "$d/defconfig"
            echo
        fi
    done
    cat "$BOARD_CFG"
    # A feature's KERNEL fragment is appended automatically, AFTER the board file
    # (kconfig takes the last assignment, and the board file is what sets the
    # base list). Getting this wrong is silent and nasty: the userspace half of a
    # feature lands, the kernel half does not, and the board boots without the
    # driver — which is exactly what happened to the `switch` feature once.
    if [ -n "$FEAT_FRAGMENTS" ]; then
        base=$(sed -n 's/^BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES="\(.*\)"$/\1/p' "$BOARD_CFG" | tail -1)
        # Deduplicate. A board defconfig may already name a feature's fragment
        # explicitly; appending it again makes Buildroot warn
        #   linux.mk:650: target '.../emmc.fragment' given more than once
        # and, worse, applies the fragment twice. Keep first occurrence, order
        # preserved (the board file's list first, features after).
        merged=""
        for f in $base $FEAT_FRAGMENTS; do
            case " $merged " in *" $f "*) continue;; esac
            merged="${merged:+$merged }$f"
        done
        echo
        echo "# --- feature kernel fragments (appended by build.sh) ---"
        echo "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"$merged\""
    fi
    # Optional downstream layer, appended LAST so its assignments win.
    #
    # This exists so a separate BR2_EXTERNAL tree can build on these boards
    # without forking this script: it would otherwise have to re-implement the
    # feature composition above, and the copy drifts the moment a board is
    # renamed or a feature added. Point OHC_EXTRA_DEFCONFIG at the extra file
    # and OHC_EXTRA_EXTERNAL at that tree's board/ directory.
    if [ -n "${OHC_EXTRA_DEFCONFIG:-}" ]; then
        [ -f "$OHC_EXTRA_DEFCONFIG" ] || {
            echo "build.sh: OHC_EXTRA_DEFCONFIG=$OHC_EXTRA_DEFCONFIG does not exist" >&2
            exit 1
        }
        echo
        echo "# --- downstream layer: $OHC_EXTRA_DEFCONFIG ---"
        cat "$OHC_EXTRA_DEFCONFIG"
    fi
} > "$EFFECTIVE"
[ -n "$FEATURES" ] && echo ">> features: $FEATURES"
echo ">> defconfig: $EFFECTIVE"

# Compose the defconfig and stop. Useful on its own — you can read exactly what
# a board resolves to without waiting for a build — and it is how the feature
# composition is regression-tested when this file changes.
if [ -n "${OHC_DEFCONFIG_ONLY:-}" ]; then
    cat "$EFFECTIVE"
    exit 0
fi

# BR2_EXTERNAL takes a space-separated list, so a downstream tree can add its
# own packages and board files alongside ours.
BR2_EXT="$REPO/board${OHC_EXTRA_EXTERNAL:+ $OHC_EXTRA_EXTERNAL}"
M=(make -C "$BUILDROOT_DIR" O="$OUT" BR2_EXTERNAL="$BR2_EXT"
   BR2_DL_DIR="$DL" BR2_JLEVEL="$JOBS")

# Wipe the output tree, keeping the generated defconfig.
#
# $EFFECTIVE is written INSIDE $OUT, so a plain `rm -rf "$OUT"` deletes the
# very file the defconfig step is about to read, and Buildroot stops with
# "Can't find default configuration". Save it across the wipe.
_wipe_out() {
    _keep=$(mktemp)
    [ -f "$EFFECTIVE" ] && cp "$EFFECTIVE" "$_keep"
    rm -rf "$OUT"
    mkdir -p "$(dirname "$EFFECTIVE")"
    [ -s "$_keep" ] && cp "$_keep" "$EFFECTIVE"
    rm -f "$_keep"
}

# Buildroot cannot change the TOOLCHAIN in an existing output tree. The
# compiler already built there was configured one way; asking for another
# quietly leaves the old one in place and the failure lands much later, in
# whatever package first needs the new capability, naming neither the toolchain
# nor the setting that changed:
#
#   musl -> glibc   : glibc's own configure dies with
#                     "cannot compute suffix of object files: cannot compile"
#   C++ enabled     : icu dies with "C++ compiler ... does not work or no
#                     compiler found" -- because there genuinely isn't one
#
# Worse, the obvious remedy does not work: `docker builder prune -af` clears
# build LAYERS but not BuildKit cache MOUNTS, and $OUT lives in a cache mount.
# It looks like it cleared and did not. (By hand: docker buildx rm sgxbuild.)
#
# So compare every toolchain-defining symbol against the tree's existing
# .config, before defconfig overwrites the evidence, and wipe on any change.
_TC_SYMS='BR2_TOOLCHAIN_BUILDROOT_GLIBC|BR2_TOOLCHAIN_BUILDROOT_MUSL|BR2_TOOLCHAIN_BUILDROOT_UCLIBC|BR2_INSTALL_LIBSTDCPP|BR2_TOOLCHAIN_BUILDROOT_CXX|BR2_USE_WCHAR|BR2_TOOLCHAIN_BUILDROOT_FORTRAN'
if [ -f "$OUT/.config" ]; then
    # LAST assignment wins, per symbol -- the same rule kconfig applies. The
    # effective defconfig is a concatenation in which common sets musl and the
    # family overrides it to glibc, so a naive grep sees BOTH and would compare
    # unequal against the resolved .config on every single build, wiping the
    # tree every time.
    # Both forms have to be read: kconfig writes an enabled symbol as
    # "SYM=y" and a disabled one as the COMMENT "# SYM is not set". Matching
    # only SYM=y means a later disable is invisible, and musl never drops out
    # when the family overrides it to glibc.
    _resolve() {
        sed -nE "s/^($_TC_SYMS)=y\$/\1 y/p; s/^# ($_TC_SYMS) is not set\$/\1 n/p" "$1" 2>/dev/null |
            awk '{ last[$1] = $2 } END { for (s in last) if (last[s] == "y") print s }' |
            sort | tr '\n' ' '
    }
    _want=$(_resolve "$EFFECTIVE")
    _have=$(_resolve "$OUT/.config")
    if [ "$_want" != "$_have" ]; then
        echo ">> toolchain change detected"
        echo ">>   was: ${_have:-<none>}"
        echo ">>   now: ${_want:-<none>}"
        echo ">> Buildroot cannot do that in place; wiping $OUT (full rebuild)"
        _wipe_out
    fi
fi

# ...and a second check, because the one above compares CONFIG to CONFIG and
# cannot see a tree whose config is already right but whose toolchain is not.
# That happens the moment a toolchain change is made and the build fails for
# some other reason: .config now says C++, the compiler on disk still has none,
# and every subsequent build agrees with itself while icu keeps insisting there
# is no C++ compiler. Ask the toolchain what it can actually do.
if [ -d "$OUT/host/bin" ] && grep -q '^BR2_INSTALL_LIBSTDCPP=y$' "$EFFECTIVE" 2>/dev/null; then
    # NOT `_cxx=$(ls ...)`: under `set -e` a command substitution whose command
    # fails takes the whole script with it, and the glob not matching is the
    # normal case on a fresh tree. Glob into a variable instead, which cannot
    # fail.
    _cxx=""
    for _c in "$OUT"/host/bin/*-g++; do
        [ -x "$_c" ] && { _cxx="$_c"; break; }
    done
    if [ -z "$_cxx" ] || ! echo 'int main(){}' | "$_cxx" -x c++ -c -o /dev/null - 2>/dev/null; then
        echo ">> config wants C++ but this toolchain has none that works"
        echo ">> wiping $OUT (full rebuild)"
        _wipe_out
    fi
fi

"${M[@]}" defconfig BR2_DEFCONFIG="$EFFECTIVE"
# Kernel bring-up iterates on both the config fragment AND the patch set
# (board/ea-common/patches/linux/). Buildroot applies patches only at EXTRACT time and
# won't re-extract a cached source, so a plain reconfigure silently ignores new
# patches. linux-dirclean wipes the extracted tree; the next build re-extracts,
# re-applies all patches, reconfigures from the fragment, and rebuilds. Drop this
# once the kernel + patches stabilize (it forces a full kernel rebuild each run).
"${M[@]}" linux-dirclean

# Force a rebuild of every package THIS REPO maintains, before the main build.
#
# Buildroot stamps each step and will not redo it, which is right for upstream
# packages whose source never changes under it. It is wrong for ours, because we
# edit both their sources and their .mk files, and the failure is silent: the
# build succeeds and quietly reinstalls what it built last time.
#
# Two different flavours of the same trap, both hit for real:
#
#   * a local-source package (SITE_METHOD=local, e.g. ohc-splash) is rsynced
#     exactly once -- .stamp_rsynced gates it -- so an edited splash.c shipped
#     the previous binary. Deleting that stamp alone is NOT enough: with
#     OVERRIDE_SRCDIR set, configure depends on it only order-only, so the copy
#     refreshes and nothing rebuilds.
#
#   * a downloaded package whose .mk we wrote (e.g. sgx545-um) never re-runs its
#     install commands, so a change to what it installs -- a patched header, a
#     new symlink -- never reaches staging. cairo then fails against the stale
#     header with an error naming neither package.
#
# <pkg>-rebuild handles both: it depends on -clean-for-rebuild, which drops the
# build and install stamps as well as the rsync one. Discovered by listing
# packages/, so a new package is covered without anyone knowing this exists.
#
# Same shape as the linux-dirclean above: a cached step that silently ignores
# changed input.
for _mk in "$REPO"/packages/*/*.mk; do
    [ -e "$_mk" ] || continue
    _pkg=$(basename "$(dirname "$_mk")")
    # only if this build selects it, else make has no such target
    _sym="BR2_PACKAGE_$(echo "$_pkg" | tr 'a-z-' 'A-Z_')"
    grep -q "^$_sym=y\$" "$OUT/.config" 2>/dev/null || continue
    echo ">> $_pkg: forcing rebuild (we maintain it; its stamps mean nothing)"
    "${M[@]}" "$_pkg-rebuild"
done

"${M[@]}" "${@:-all}"

echo ">> image: $OUT/images/openhc-$BOARD-kernel.img"
