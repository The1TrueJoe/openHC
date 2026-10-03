#!/usr/bin/env bash
# Build the openHC daemons for a board and stage them into its rootfs overlay.
#
#   packages/build.sh <board>     (default: ca1)
#
# <board> is a DIRECTORY NAME under board/, because that is what CI passes: the
# matrix is built from the board tree, so it says "ea1-v2-poe", not "ea1". The
# case below matched families only, which meant every EA board died here with
# "unknown board" and shipped an image with no iod, no webd and no sysmond. The
# glob patterns are what keep a new variant from reintroducing that.
#
# Cross-compiles webd (Rust, with the React UI embedded) for the board's
# arch and drops it at board/common/rootfs-overlay/opt/ohc/bin/webd, which
# the Buildroot image then bundles. The Mac's Homebrew rustc has no cross std, so
# we resolve a cargo whose toolchain does (rustup's) and pin RUSTC beside it.
set -euo pipefail
BOARD="${1:-ca1}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

case "$BOARD" in
  ca1*)             TARGET=armv7-unknown-linux-musleabihf ;;
  # EA runs a modern 64-bit userspace. The Buildroot rootfs is x86_64 (glibc) and
  # the daemons are x86_64 static-musl, so they match the base and the IA32
  # emulation layer is reserved for the one thing that genuinely needs it: the
  # 32-bit PowerVR SGX545 GLES/WPE sub-stack (no 64-bit DDK exists for this core).
  # Static musl has no loader dependency, so it runs cleanly on the glibc rootfs.
  ea1*|ea3*|ea5*)   TARGET=x86_64-unknown-linux-musl ;;
  # The HC-800 is x86_64 too (Control4 shipped a 32-bit kernel on the same silicon).
  hc800*)           TARGET=x86_64-unknown-linux-musl ;;
  ioxv1*)           TARGET=armv5te-unknown-linux-musleabi ;;
  *) echo "build.sh: unknown board '$BOARD'"; exit 1 ;;
esac

# Every target needs a [target.*] block in packages/.cargo/config.toml naming a
# linker. Without one cargo reaches for the host `cc`, which on a 64-bit runner
# greets a 32-bit or ARM crt1.o with "file in wrong format" — a link error that
# reads like a broken toolchain rather than a missing three-line config. Fail
# here instead, where the message can say what to add.
if ! grep -q "^\[target\.$TARGET\]" "$HERE/.cargo/config.toml" 2>/dev/null; then
  echo "build.sh: no [target.$TARGET] in packages/.cargo/config.toml" >&2
  echo "  cargo would fall back to the host linker and fail on the object format" >&2
  exit 1
fi

# a cargo whose toolchain actually has std for $TARGET (Homebrew's lies about it)
pick_cargo() {
  for c in "$HOME/.cargo/bin/cargo" "$HOME"/.rustup/toolchains/*/bin/cargo; do
    [ -x "$c" ] || continue
    rc="$(dirname "$c")/rustc"
    sys="$("$rc" --print sysroot 2>/dev/null)" || continue
    [ -d "$sys/lib/rustlib/$TARGET" ] && { echo "$c"; return; }
  done
  echo "build.sh: no cargo toolchain has std for $TARGET" >&2
  echo "  run: rustup target add $TARGET" >&2
  exit 1
}
CARGO="$(pick_cargo)"
export RUSTC="$(dirname "$CARGO")/rustc"

# --- which crates this board actually gets --------------------------------------
# CORE daemons ship on every board. FEATURE daemons ship only where the board's
# ohc.features enables the owning feature — the same gate build/build.sh uses for
# kernel config and Buildroot packages, extended to our Rust crates, so an IO
# Extender does not carry the Zigbee binary and a CA-1 does not carry audio.
#
# A feature owns a crate by dropping a `packages` file in its feature directory,
# one "<crate> <binary>" per line (binary defaults to the crate name). The family
# base and feature search mirror build/build.sh: ea* -> ea/common, and the EA
# boards live at board/ea/<board>, so BOARD_DIR (not board/$BOARD) is the board.
case "$BOARD" in
  ea*) FAM="$REPO/board/ea/common"; BOARD_DIR="$REPO/board/ea/$BOARD" ;;
  *)   FAM="";                      BOARD_DIR="$REPO/board/$BOARD" ;;
esac
FEATURES=""
for ff in ${FAM:+"$FAM/ohc.features"} "$BOARD_DIR/ohc.features"; do
  [ -f "$ff" ] && FEATURES="$FEATURES $(sed 's/#.*//' "$ff" | tr '\n' ' ')"
done
FEATURES=$(printf '%s\n' $FEATURES | awk 'NF && !seen[$0]++' | tr '\n' ' ')

feature_dir() {
  for d in "$REPO/board/common/features/$1" ${FAM:+"$FAM/features/$1"} "$BOARD_DIR/features/$1"; do
    [ -d "$d" ] && { echo "$d"; return 0; }
  done
  return 1
}

# Core crates/binaries, on every board. The `portal` binary now lives in the
# `wifi` crate (merged with the shared Wi-Fi lib), so we build crate `wifi` to
# get it; the staged binary is still `portal`. CRATES (what `cargo build -p`
# builds) and BINS (what gets installed) are separate lists, so a crate whose
# binary has a different name is expressed by listing each accordingly.
CRATES="iod webd wifi sysmond"   # core
BINS="iod webd portal sysmond"
for f in $FEATURES; do
  d=$(feature_dir "$f") || continue
  [ -f "$d/packages" ] || continue
  while read -r crate bin _rest; do
    case "$crate" in ''|\#*) continue ;; esac
    CRATES="$CRATES $crate"
    BINS="$BINS ${bin:-$crate}"
    echo ">> feature '$f' adds crate '$crate' (bin ${bin:-$crate})"
  done < "$d/packages"
done

echo ">> UI (must build before cargo — build.rs embeds ui/dist)"
( cd "$HERE/webd/ui" && npm ci --no-audit --no-fund 2>/dev/null || npm install --no-audit --no-fund; npm run build )

echo ">> crates for $BOARD ($TARGET): $CRATES"
( cd "$HERE" && "$CARGO" build --release $(for c in $CRATES; do printf ' -p %s' "$c"; done) --target "$TARGET" )

DEST="$REPO/board/common/rootfs-overlay/opt/ohc/bin"
mkdir -p "$DEST"
for bin in $BINS; do
    install -m 0755 "$HERE/target/$TARGET/release/$bin" "$DEST/$bin"
    echo ">> staged $DEST/$bin ($(du -h "$DEST/$bin" | cut -f1))"
done
echo ">> now: make image BOARD=$BOARD"
