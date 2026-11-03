#!/usr/bin/env bash
# fetch.sh <dest> <sha256> <url>...: download from the first URL that answers,
# retrying each, and refuse a file whose SHA-256 is not <sha256>.
set -euo pipefail
dest="${1:?dest}" sum="${2:?sha256}"; shift 2
for url in "$@"; do
    if wget -q --tries=3 --waitretry=10 --retry-connrefused --timeout=60 -O "$dest" "$url"; then
        if echo "$sum  $dest" | sha256sum -c - >/dev/null 2>&1; then
            echo ">> $(basename "$dest") from $url"
            exit 0
        fi
        echo "::warning title=fetch::$url: SHA-256 mismatch"
    else
        echo "::warning title=fetch::$url did not answer"
    fi
done
rm -f "$dest"
echo "::error title=fetch::no mirror served $(basename "$dest") with SHA-256 $sum"
exit 1
