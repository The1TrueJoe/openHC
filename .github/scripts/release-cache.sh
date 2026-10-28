#!/usr/bin/env bash
# A directory cache kept as assets on the `ci-cache` pre-release, for what the
# 10 GB Actions cache cannot hold: every board's Buildroot output tree and the
# WebKit boards' ccache. An HC-800 tree alone (WebKit, LLVM, Node) outgrows what
# the cap leaves once the other boards are in, so it was evicted, every PR built
# Node and WebKit cold, and the job ran out of time. Release assets have no
# total cap and no expiry; one asset is capped at 2 GiB, so an entry is split
# into parts and a manifest lists them (RELEASE_CACHE_PART, default 1900M).
#
#   release-cache.sh save    <name> <dir>   tar+zstd <dir>, replace <name>
#   release-cache.sh restore <name> <dir>   unpack <name> into <dir>, if present
#
# A restore that finds nothing, or fails part-way, leaves <dir> absent and exits
# 0: the build then starts cold, which is slow but right. Only main saves (see
# ci.yml), and main never runs two image builds at once, so there is one writer.
# The manifest is deleted before the parts are replaced and uploaded after, so a
# reader never follows a manifest into parts that are mid-replace.
#
# The release is a pre-release, so /releases/latest (the stable channel, the
# flasher) never sees it, and its asset names match nothing the dev channel
# reads. Needs GH_TOKEN; save needs contents: write.
set -euo pipefail

TAG=ci-cache
R="${GITHUB_REPOSITORY:?}"
op="${1:?save|restore}" name="${2:?name}" dir="${3:?dir}"

case "$op" in
restore)
    tmp=$(mktemp -d)
    if ! gh release download "$TAG" -R "$R" -p "$name.manifest" -D "$tmp" 2>/dev/null; then
        echo ">> $name: not in the $TAG release, starting cold"
        exit 0
    fi
    mkdir -p "$dir"
    # Streamed part by part: the compressed tree never sits on the runner's disk
    # next to the unpacked one.
    if while read -r part; do gh release download "$TAG" -R "$R" -p "$part" -O -; done < "$tmp/$name.manifest" \
        | zstd -dc | tar -C "$dir" -x; then
        echo ">> $name: restored ($(du -sh "$dir" | cut -f1))"
    else
        echo "::warning title=release-cache::$name did not unpack; starting cold"
        rm -rf "$dir"
    fi
    ;;
save)
    [ -d "$dir" ] || { echo ">> $name: no $dir, nothing to save"; exit 0; }
    gh release view "$TAG" -R "$R" >/dev/null 2>&1 ||
        gh release create "$TAG" -R "$R" --prerelease --latest=false --title "CI cache" \
            --notes "Build caches for CI (Buildroot output trees, ccache), written by main's image builds. Not a release: nothing here is meant to be downloaded by hand."
    gh release delete-asset "$TAG" "$name.manifest" -R "$R" -y 2>/dev/null || true
    gh release view "$TAG" -R "$R" --json assets -q '.assets[].name' | { grep -F "$name.part-" || true; } |
        while read -r a; do gh release delete-asset "$TAG" "$a" -R "$R" -y; done
    work=$(mktemp -d)
    # Each part is uploaded and deleted as soon as split finishes it, so at most
    # one part is on disk at a time. $FILE is split's, expanded by its filter shell.
    # shellcheck disable=SC2016
    tar -C "$dir" -cf - . | zstd -T0 -3 -q |
        split -b "${RELEASE_CACHE_PART:-1900M}" -d -a 2 --filter='cat > "$FILE" && gh release upload '"$TAG"' "$FILE" -R '"$R"' --clobber && rm "$FILE"' \
            - "$work/$name.part-"
    gh release view "$TAG" -R "$R" --json assets -q '.assets[].name' | { grep -F "$name.part-" || true; } | sort > "$work/$name.manifest"
    [ -s "$work/$name.manifest" ] || { echo "::error title=release-cache::$name uploaded no parts"; exit 1; }
    gh release upload "$TAG" "$work/$name.manifest" -R "$R" --clobber
    echo ">> $name: saved as $(wc -l < "$work/$name.manifest") part(s)"
    ;;
*)
    echo "usage: $0 save|restore <name> <dir>" >&2
    exit 2
    ;;
esac
