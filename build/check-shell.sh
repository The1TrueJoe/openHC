#!/usr/bin/env bash
# Every board.env and init script must at least PARSE. board.env is sourced by
# nearly every init script (networking included), so one bad line there yields
# an image that boots to a login prompt with no network. Seconds, so CI runs it
# before any multi-hour build.
set -e
cd "$(dirname "$0")/.."
bad=0
while IFS= read -r f; do
  sh -n "$f" || { echo "::error file=$f::shell syntax error"; bad=1; }
done < <(git ls-files 'board/**/board.env' 'board/**/etc/init.d/*' 'board/**/opt/ohc/bin/*' 'board/**/opt/ohc/libexec/*' \
         | while read -r f; do head -c 64 "$f" | grep -q '^#!.*sh' && echo "$f" || case "$f" in (*board.env) echo "$f";; esac; done)
exit $bad
