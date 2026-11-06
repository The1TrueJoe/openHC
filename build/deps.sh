#!/usr/bin/env bash
# Host packages for a native Buildroot build on Debian/Ubuntu: the same list
# CI and build/Dockerfile install.
#
#   build/deps.sh [--kexec]   --kexec adds the 32-bit toolchain the HC-800's
#                             static i686 kexec needs (build/kexec-i686.sh)
#   build/deps.sh --print     just print the list (other distros: install the
#                             equivalents by hand)
set -euo pipefail
pkgs="build-essential bc bison flex git rsync unzip wget cpio file figlet
      python3 libncurses-dev libssl-dev libelf-dev pkg-config ca-certificates perl patch xz-utils zip"
for a in "$@"; do
  case "$a" in
    --kexec) pkgs="$pkgs gcc-multilib libc6-dev-i386" ;;
    --print) echo $pkgs; exit 0 ;;
    *) echo "usage: $0 [--kexec] [--print]" >&2; exit 2 ;;
  esac
done
command -v apt-get >/dev/null || { echo "no apt-get: install these yourself: $(echo $pkgs)" >&2; exit 1; }
sudo=; [ "$(id -u)" = 0 ] || sudo=sudo
$sudo apt-get update -qq
# shellcheck disable=SC2086
$sudo apt-get install -y --no-install-recommends $pkgs
