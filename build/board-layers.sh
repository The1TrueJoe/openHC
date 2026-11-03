# Sourced by build/build.sh and packages/build.sh. Needs REPO and BOARD; sets
# FAMILY_DIR, MODEL_DIR, BOARD_DIR, SCOPES and FEATURES, defines feature_dirs.
#
# Layers, most general first; each later one extends or overrides the earlier:
#   board/common              every board
#   board/ea/common           the EA family (Intel CE5300)
#   board/ea/<model>          one model: its .dtsi, features, defconfig, board.env
#   board/ea/<model>/<board>  one variant: its .dts and what differs
# A standalone board (hc800, ca1, ioxv1) is board/common + board/<board>.
FAMILY_DIR=""; MODEL_DIR=""
case "$BOARD" in
  # Standalone i386 WPE/SGX tree, not a bootable board (no family base).
  gpu-wpe-i386) BOARD_DIR="$REPO/board/ea/common/gpu-wpe-i386" ;;
  ea*)
    FAMILY_DIR="$REPO/board/ea/common"
    BOARD_DIR=$(ls -d "$REPO"/board/ea/*/"$BOARD" 2>/dev/null | head -1)
    [ -n "$BOARD_DIR" ] && MODEL_DIR=$(dirname "$BOARD_DIR") ;;
  *) BOARD_DIR="$REPO/board/$BOARD" ;;
esac
SCOPES="$REPO/board/common $FAMILY_DIR $MODEL_DIR $BOARD_DIR"

# A feature is a directory, features/<name>/, holding any of defconfig,
# linux.fragment, rootfs-overlay/, board.env, packages and requires.
# Every scope's copy of a feature, most general first.
feature_dirs() {
    _found=1
    for _s in $SCOPES; do
        [ -d "$_s/features/$1" ] && { printf '%s\n' "$_s/features/$1"; _found=0; }
    done
    return $_found
}

# Features: words in each scope's ohc.features ('#' comments), then whatever
# their features/<name>/requires pull in.
FEATURES=""
for _s in $SCOPES; do
    [ "$_s" = "$REPO/board/common" ] && continue
    [ -f "$_s/ohc.features" ] && FEATURES="$FEATURES $(sed 's/#.*//' "$_s/ohc.features" | tr '\n' ' ')"
done
FEATURES=$(printf '%s\n' $FEATURES | awk 'NF && !seen[$0]++' | tr '\n' ' ')
_req_round=0
while [ $_req_round -lt 8 ]; do
    _more=""
    for _f in $FEATURES; do
        for _d in $(feature_dirs "$_f" || true); do
            [ -f "$_d/requires" ] && _more="$_more $(sed 's/#.*//' "$_d/requires" | tr '\n' ' ')"
        done
    done
    _new=$(printf '%s\n' $_more $FEATURES | awk 'NF && !seen[$0]++' | tr '\n' ' ')
    [ "$_new" = "$FEATURES" ] && break
    FEATURES=$_new; _req_round=$((_req_round + 1))
done
