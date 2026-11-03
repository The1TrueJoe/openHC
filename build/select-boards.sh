#!/usr/bin/env bash
# Which boards a change rebuilds: images.yml's `select` job, runnable locally.
#
#   build/select-boards.sh [BASE]
#
#   BASE          the commit HEAD is compared with (a PR's base, a push's
#                 before). Missing or unknown: every board.
#   EVENT         pull_request (default) or push. flasher/crates/core rebuilds
#                 images on push only; any other event builds every board.
#   SELECT_BOARD  build just this board.
#
# Prints all=[...] and boards=[...] (JSON) on stdout, the reasons on stderr.
# Locally: build/select-boards.sh origin/main
# Board and crate names are plain words: the unquoted splits are on purpose.
# shellcheck disable=SC2010,SC2046,SC2206
set -euo pipefail
cd "$(dirname "$0")/.."
EVENT="${EVENT:-pull_request}"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
exec 3>&1 1>&2

# The fleet is every board directory with a <name>/<name>_defconfig.
boards_in() { for d in "$@"; do n=$(basename "$d"); [ "$n" != common ] && [ -f "$d/${n}_defconfig" ] && echo "$n"; done; return 0; }
EA_BOARDS=$(boards_in $(ls -d board/ea/*/*/ | grep -v '^board/ea/common/') | tr '\n' ' ')
OTHER_BOARDS=$(boards_in board/*/ | tr '\n' ' ')
printf -v ALL '"%s",' $EA_BOARDS $OTHER_BOARDS
ALL="[${ALL%,}]"
echo "all=$ALL" >&3

# Explicit request, or a release/scheduled/dispatch run with no board:
# build all.
if [ -n "${SELECT_BOARD:-}" ]; then
  echo "boards=[\"${SELECT_BOARD:-}\"]" >&3; exit 0
fi
if [ "$EVENT" != "push" ] && [ "$EVENT" != "pull_request" ]; then
  echo "boards=$ALL" >&3; exit 0
fi

base="${1:-}"
# First push to a branch has no usable base; build everything.
if [ -z "$base" ] || [ "$base" = "0000000000000000000000000000000000000000" ] \
   || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  echo "boards=$ALL" >&3; exit 0
fi
# Only this change, even when BASE has moved on (a local origin/main).
base=$(git merge-base "$base" HEAD 2>/dev/null || echo "$base")
changed=$(git diff --name-only "$base" HEAD)
echo "changed files:"; echo "$changed" | sed 's/^/  /'

# Shared trees affect every board. packages/ and flasher/ are scoped
# further down to the boards that ship what changed.
if echo "$changed" | grep -qE '^(board/common/|board/Config\.in|board/external\.|build/|Makefile)'; then
  echo "boards=$ALL" >&3; exit 0
fi

sel=()

# flasher/ is its own build (the flasher job). An image carries only
# flasher/crates/core, compiled into the restore and update daemons; CI
# never runs ohc-flash in an image build (post-image skips the netboot
# wrap without it). A PR's daemons-build already compiles those
# daemons, so only main rebuilds the images that ship them.
if [ "$EVENT" = push ] && echo "$changed" | grep -q '^flasher/crates/core/'; then
  for b in $EA_BOARDS $OTHER_BOARDS; do
    case " $(bash packages/build.sh "$b" --crates 2>/dev/null) " in
      *" restore "*|*" update "*) sel+=("$b") ;;
    esac
  done
fi

# packages/ — the Rust daemons, staged into the overlay. A change here
# used to rebuild all eight images; instead map it to the boards that
# ship the touched crate. packages/build.sh <board> --crates lists the
# top-level crates a board gets (core iod/webd/sysmond + its features'),
# resolved from ohc.features with no build, so it is cheap to ask here.
pkgchanged=$(echo "$changed" | grep -E '^packages/' || true)
if [ -n "$pkgchanged" ]; then
  # Workspace-wide inputs rebuild every board: the workspace manifest,
  # the shared cargo/toolchain config, and the staging driver itself
  # all change what every board compiles. Cargo.lock is NOT here: a
  # lockfile change is resolved to the crates it touches (below).
  if echo "$pkgchanged" | grep -qE '^packages/(Cargo\.toml|\.cargo/|rust-toolchain|build\.sh|README)'; then
    echo ">> packages/: workspace-wide file changed — building all"
    echo "boards=$ALL" >&3; exit 0
  fi
  lockchanged=""
  echo "$pkgchanged" | grep -qx 'packages/Cargo\.lock' && lockchanged=1
  # The crate each changed file lives in (first path component). A file
  # sitting directly under packages/ is not attributable to one crate,
  # so treat it as workspace-wide and build everything.
  crates=""
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    [ "$f" = "packages/Cargo.lock" ] && continue
    c=$(printf '%s\n' "$f" | sed -n 's#^packages/\([^/]*\)/.*#\1#p')
    [ -z "$c" ] && { echo ">> packages/: unattributable file '$f' — building all"; echo "boards=$ALL" >&3; exit 0; }
    crates="$crates $c"
  done <<EOF
$pkgchanged
EOF
  # Each board's shipped crates, asked once, as padded lines
  # "<board>  <crate> <crate> " so a whole-word " <crate> " grep cannot
  # match a board name or a crate substring. No associative arrays —
  # the select runner is bash but this stays portable regardless.
  ships_table=$(for b in $EA_BOARDS $OTHER_BOARDS; do
    printf '%s  %s \n' "$b" "$(bash packages/build.sh "$b" --crates 2>/dev/null)"
  done)
  # A dependency bump touches only Cargo.lock. Rather than rebuild every
  # board, diff the lockfile base..head and keep the shipped crates whose
  # TRANSITIVE resolved dependency set changed (a core-crate dep bump hits
  # iod/webd/sysmond, so every board; a storage-only dep hits only
  # storage boards). It is a lockfile parse, not `cargo metadata`, so the
  # select job needs no Rust toolchain; see the script header. Any
  # failure to read either lockfile falls back to building all.
  if [ -n "$lockchanged" ]; then
    shipped=$(printf '%s\n' "$ships_table" | awk '{for(i=2;i<=NF;i++)print $i}' | sort -u | tr '\n' ' ')
    git show "$base:packages/Cargo.lock" > "$tmp/Cargo.lock.base" 2>/dev/null || true
    if [ ! -s "$tmp/Cargo.lock.base" ] \
       || ! lockcrates=$(python3 .github/scripts/lock-impacted-crates.py "$tmp/Cargo.lock.base" packages/Cargo.lock $shipped); then
      echo ">> packages/Cargo.lock: could not resolve the change — building all"
      echo "boards=$ALL" >&3; exit 0
    fi
    echo ">> Cargo.lock change impacts crates: ${lockcrates:-none}"
    crates="$crates $lockcrates"
  fi
  crates=$(printf '%s\n' $crates | awk 'NF' | sort -u | tr '\n' ' ')
  echo ">> changed package crates: $crates"
  for c in $crates; do
    # || true: a no-match grep under `set -o pipefail` would otherwise
    # abort the whole step before the shared-dependency fallback below
    # — and that silent abort resolves to an EMPTY board list, i.e. a
    # crate change that builds nothing. The empty hits must reach the
    # `[ -z ]` test, not kill the script.
    hits=$(printf '%s\n' "$ships_table" | grep -F " $c " | awk '{print $1}' || true)
    # A crate no board lists via --crates is a shared DEPENDENCY (e.g.
    # librespot, alsa-ohceq, ohcmqtt — pulled in by audiod/iod, never
    # built with their own -p). We cannot cheaply know which top-level
    # crates depend on it, so be safe and build everything.
    if [ -z "$hits" ]; then
      echo ">> crate '$c' is a shared dependency (no board ships it directly) — building all"
      echo "boards=$ALL" >&3; exit 0
    fi
    for b in $hits; do sel+=("$b"); done
  done
fi
# ea/common is shared by the EA family only.
if echo "$changed" | grep -qE '^board/ea/common/'; then
  sel+=($EA_BOARDS)
fi
# The DSP program's workflow goes into the EA3 images (the `dsp` job).
if echo "$changed" | grep -qx '.github/workflows/dsp-program.yml'; then
  sel+=(ea3-v1 ea3-v2)
fi
# EA boards live at board/ea/<model>/<board>/; a change in the model
# folder outside any board folder rebuilds every board of that model.
for b in $EA_BOARDS; do
  m=$(basename "$(dirname board/ea/*/"$b")")
  echo "$changed" | grep -qE "^board/ea/$m/$b/" && sel+=("$b")
  echo "$changed" | grep -E "^board/ea/$m/" | grep -vqE "^board/ea/$m/$m-[^/]+/" && sel+=("$b")
done
for b in $OTHER_BOARDS; do
  echo "$changed" | grep -qE "^board/$b/" && sel+=("$b")
done
# Workflow-only or docs-only change: nothing to build.
if [ ${#sel[@]} -eq 0 ]; then
  echo "boards=[]" >&3; exit 0
fi
printf -v j '"%s",' $(printf '%s\n' "${sel[@]}" | sort -u)
echo "boards=[${j%,}]" >&3
