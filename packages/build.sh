#!/usr/bin/env bash
# Build the openHC daemons for a board and stage them into its rootfs overlay.
#
#   packages/build.sh [ca1|ea1|ea3|...]     (default: ca1)
#
# Cross-compiles ohc-webd (Rust, with the React UI embedded) for the board's
# arch and drops it at board/common/rootfs-overlay/opt/ohc/bin/ohc-webd, which
# the Buildroot image then bundles. The Mac's Homebrew rustc has no cross std, so
# we resolve a cargo whose toolchain does (rustup's) and pin RUSTC beside it.
set -euo pipefail
BOARD="${1:-ca1}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

case "$BOARD" in
  ca1)          TARGET=armv7-unknown-linux-musleabihf ;;
  ea1|ea3|ea5)  TARGET=i686-unknown-linux-musl ;;
  *) echo "build.sh: unknown board '$BOARD'"; exit 1 ;;
esac

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

echo ">> UI (must build before cargo — build.rs embeds ui/dist)"
( cd "$HERE/ohc-webd/ui" && npm ci --no-audit --no-fund 2>/dev/null || npm install --no-audit --no-fund; npm run build )

echo ">> ohc-webd + ohc-portal for $BOARD ($TARGET)"
( cd "$HERE" && "$CARGO" build --release -p ohc-webd -p ohc-portal --target "$TARGET" )

DEST="$REPO/board/common/rootfs-overlay/opt/ohc/bin"
mkdir -p "$DEST"
for bin in ohc-webd ohc-portal; do
    install -m 0755 "$HERE/target/$TARGET/release/$bin" "$DEST/$bin"
    echo ">> staged $DEST/$bin ($(du -h "$DEST/$bin" | cut -f1))"
done
echo ">> now: make image BOARD=$BOARD"
