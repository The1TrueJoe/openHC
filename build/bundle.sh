#!/usr/bin/env bash
# Zip a built board's images/ into the bundle the flasher reads
# (Release::open looks for the canonical names inside: bzImage, rootfs.ext2,
# ...). Only images/: the rest of output/ is the multi-GB build tree.
#
#   build/bundle.sh BOARD VERSION   writes openhc-BOARD-VERSION.zip in the repo
#                                   root and prints its name
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BOARD="${1:?usage: $0 BOARD VERSION}"; VERSION="${2:?usage: $0 BOARD VERSION}"
# build.sh keeps a per-board output tree, output/build/$BOARD.
imgdir="$REPO/output/build/$BOARD/images"
[ -d "$imgdir" ] || { echo "no images at $imgdir: build $BOARD first" >&2; exit 1; }
bundle="openhc-$BOARD-$VERSION.zip"
rm -f "$REPO/$bundle"
ls -l "$imgdir" >&2
( cd "$imgdir" && zip -q -r -X "$REPO/$bundle" . )
ls -l "$REPO/$bundle" >&2
echo "$bundle"
