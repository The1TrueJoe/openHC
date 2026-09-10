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
  ea1*|ea3*|ea5*)   TARGET=i686-unknown-linux-musl ;;
  # The HC-800 is the one 64-bit board: openHC builds it x86_64 even though
  # Control4 shipped a 32-bit kernel on the same silicon.
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

echo ">> UI (must build before cargo — build.rs embeds ui/dist)"
( cd "$HERE/webd/ui" && npm ci --no-audit --no-fund 2>/dev/null || npm install --no-audit --no-fund; npm run build )

echo ">> iod + webd + portal + sysmond for $BOARD ($TARGET)"
( cd "$HERE" && "$CARGO" build --release -p iod -p webd -p portal -p sysmond --target "$TARGET" )

DEST="$REPO/board/common/rootfs-overlay/opt/ohc/bin"
mkdir -p "$DEST"
for bin in iod webd portal sysmond; do
    install -m 0755 "$HERE/target/$TARGET/release/$bin" "$DEST/$bin"
    echo ">> staged $DEST/$bin ($(du -h "$DEST/$bin" | cut -f1))"
done
echo ">> now: make image BOARD=$BOARD"
