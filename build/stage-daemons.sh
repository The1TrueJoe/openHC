#!/usr/bin/env bash
# Cross-compile a board's openHC daemons and stage them into the rootfs overlay
# (board/common/rootfs-overlay/opt/ohc/bin), where the image build picks them
# up. Without it the image ships no iod and no webd: the box boots with no IO
# server and no GUI.
#
#   build/stage-daemons.sh BOARD
#
# Needs rustup, node/npm (the webd UI) and the target's C compiler, which ring
# compiles a few shims with; on Debian/Ubuntu that compiler is installed here.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BOARD="${1:?usage: $0 BOARD}"
# Globs, so a future hc800-v2 or ca1-v2 does not fall through to the EA default
# and get silently cross-compiled for the wrong arch. This is only the C-shim
# compiler and its headers; which LINKER each target uses lives in
# packages/.cargo/config.toml, and packages/build.sh refuses a target with no
# entry there.
case "$BOARD" in
  hc800*) target=x86_64-unknown-linux-musl;      cc=musl-gcc;                apt="musl-tools" ;;
  ca1*)   target=armv7-unknown-linux-musleabihf; cc=arm-linux-gnueabihf-gcc; apt="gcc-arm-linux-gnueabihf libc6-dev-armhf-cross" ;;
  ioxv1*) target=armv5te-unknown-linux-musleabi; cc=arm-linux-gnueabi-gcc;   apt="gcc-arm-linux-gnueabi libc6-dev-armel-cross" ;;
  # EA default: a modern 64-bit userspace (x86_64 static-musl daemons).
  *)      target=x86_64-unknown-linux-musl;      cc=musl-gcc;                apt="musl-tools" ;;
esac
if ! command -v "$cc" >/dev/null; then
  command -v apt-get >/dev/null || { echo "need $cc for $target (Debian: $apt)" >&2; exit 1; }
  sudo=; [ "$(id -u)" = 0 ] || sudo=sudo
  $sudo apt-get update -qq
  # shellcheck disable=SC2086
  $sudo apt-get install -y --no-install-recommends $apt
fi
rustup target add "$target"
export "CC_$(echo "$target" | tr '-' '_')=$cc"
bash "$REPO/packages/build.sh" "$BOARD"
ls -l "$REPO/board/common/rootfs-overlay/opt/ohc/bin/"
