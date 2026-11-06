#!/usr/bin/env bash
# Buildroot at the pinned ref (build/buildroot.ref), with build/buildroot-patches
# applied: patches to Buildroot ITSELF (a .mk or a Config.in), as opposed to
# BR2_GLOBAL_PATCH_DIR, which patches package sources. --forward, so a patch
# upstream has since adopted is skipped rather than fatal.
#
#   build/fetch-buildroot.sh [DIR]     DIR defaults to $BUILDROOT_DIR, then ~/buildroot
#
# A DIR already fetched for this ref and patch set is left alone.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
DIR="${1:-${BUILDROOT_DIR:-$HOME/buildroot}}"
REF=$(tr -d '[:space:]' < "$REPO/build/buildroot.ref")
stamp="$REF $(cat "$REPO"/build/buildroot-patches/*.patch 2>/dev/null | git hash-object --stdin)"
if [ "$(cat "$DIR/.openhc-fetched" 2>/dev/null)" = "$stamp" ]; then
  echo ">> buildroot: $DIR already at $REF"; exit 0
fi
rm -rf "$DIR"
git init -q "$DIR"
# A commit or a tag both work.
git -C "$DIR" fetch -q --depth 1 https://github.com/buildroot/buildroot "$REF"
git -C "$DIR" checkout -q FETCH_HEAD
for p in "$REPO"/build/buildroot-patches/*.patch; do
  [ -e "$p" ] || continue
  echo ">> buildroot patch: ${p##*/}"
  patch -d "$DIR" -p1 --forward --batch < "$p"
done
echo "$stamp" > "$DIR/.openhc-fetched"
echo ">> buildroot: $DIR at $REF"
