#!/bin/sh
# Fetch the pinned EM357 NCP image (em357-ncp.source) and install it where
# ohc-zigbee looks for it. Run by board/common/post-build.sh on boards that ship
# ohc-zigbee.
#
#   install.sh <target-dir> <download-dir>
#
# The download is cached in <download-dir> (Buildroot's dl/, which CI caches) and
# checked against the pinned sha256 every time; a mismatch or a failed download
# fails the build rather than shipping a Zigbee image that cannot reflash.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
TARGET="$1"; DL="$2/em357-ncp"
. "$HERE/em357-ncp.source"

mkdir -p "$DL"
f="$DL/$EM357_NCP_FILE"
ok() { [ -f "$f" ] && echo "$EM357_NCP_SHA256  $f" | sha256sum -c - >/dev/null 2>&1; }
if ! ok; then
    rm -f "$f" "$f.part"
    if command -v wget >/dev/null; then
        wget -q -O "$f.part" "$EM357_NCP_URL"
    else
        curl -fsSL -o "$f.part" "$EM357_NCP_URL"
    fi
    mv "$f.part" "$f"
    ok || { echo "em357: $EM357_NCP_FILE does not match its pinned sha256" >&2; rm -f "$f"; exit 1; }
fi

dest="$TARGET/usr/share/ohc/zigbee"
mkdir -p "$dest"
install -m 0644 "$f" "$dest/em357-ncp.ebl"
echo "$EM357_NCP_STACK" > "$dest/em357-ncp.ebl.version"
echo "em357: installed EmberZNet $EM357_NCP_STACK NCP image"
