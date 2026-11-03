#!/usr/bin/env bash
# The version stamped into an image (/etc/openhc-release) and the flasher, and
# used in bundle names: a release's tag, else <short-sha>-dev.
#
#   build/version.sh [TAG]
set -euo pipefail
if [ -n "${1:-}" ]; then echo "$1"; else echo "$(git -C "$(dirname "$0")/.." rev-parse --short HEAD)-dev"; fi
