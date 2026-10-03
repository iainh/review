#!/usr/bin/env bash
set -euo pipefail

# Disposable native Sway session, 1280×900 scale 1 (tests/sway.conf).
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[title=\"^Review — huge.pdf\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_tiles_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
geometry=$(realpath tests/pdf-page-geometry.py)
swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/huge.pdf'" >/dev/null

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review — huge.pdf")) | .name'
}
expect_zoom() {
    for _ in {1..100}; do
        if [[ $(title) == *" — 1/1 — $1" ]]; then
            echo "PASS: huge page at $1"
            sleep .7
            return
        fi
        sleep .1
    done
    echo "Expected $1; got $(title)" >&2
    exit 1
}
key() { wtype -s 150 -k "$1" -s 150; }
command() { wtype -s 150 -M ctrl -k "$1" -m ctrl -s 150; }
zoom() { wtype -s 150 -M ctrl -k l -m ctrl -s 150 "$1" -s 150 -k Return -s 150; expect_zoom "$1%"; }
page_geometry() {
    rm -f "$scratch/page.json"
    swaymsg "exec /usr/bin/python3 '$geometry' '$scratch/page.json' 'PDF page 1' > '$scratch/geometry.log' 2>&1" >/dev/null
    for _ in {1..100}; do [[ -f "$scratch/page.json" ]] && return; sleep .1; done
    cat "$scratch/geometry.log" >&2
    exit 1
}
pointer() {
    local x y
    read -r x y < <(/usr/bin/python3 - "$scratch/page.json" "$2" "$3" <<'PY'
import json, sys
x, y, width, height = json.load(open(sys.argv[1]))
print(round(x + float(sys.argv[2]) * width / 20000),
      round(y + float(sys.argv[3]) * height / 12000))
PY
    )
    "$scratch/pointer" "$1" "$x" "$y"
}
clipboard() {
    for _ in {1..30}; do
        [[ $(wl-paste --no-newline 2>/dev/null || true) == "$1" ]] && { echo "PASS: selected $1"; return; }
        sleep .1
    done
    echo "Unexpected clipboard: $(wl-paste --no-newline 2>/dev/null || true)" >&2
    exit 1
}
capture() {
    sleep .5
    grim "$scratch/$1.png"
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        cp "$scratch/$1.png" "$REVIEW_SCREENSHOTS/tiles-$1.png"
    fi
}

expect_zoom 'Fit page'
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review — huge.pdf")) | .shell') == xdg_shell ]]
key F9 # Give the cross-tile text the full viewport width.
key 1
expect_zoom '100%'
page_geometry
capture initial
# Exclude shell/toolbars whose history controls legitimately change state.
# Use the native PDF bounds rather than a fixed pre-titlebar crop offset.
page_top=$(jq -r '.[1] | ceil' "$scratch/page.json")
page_crop="1240x$((890 - page_top))+10+$page_top"
pointer double 740 267 # "crossing" straddles the 768-point / 1024-pixel tile edge.
command c
clipboard crossing
capture selection
key Escape
pointer click 130 116 # Internal destination far outside the initial raster.
expect_zoom '100%'
page_geometry
pointer double 4020 2991
command c
clipboard Distant
capture link-destination
key Escape
"$scratch/pointer" scroll 600 400 350
capture pan
wtype -s 150 -M alt -k Left -m alt -s 150
expect_zoom '100%'
capture back
# History must restore the same visible raster, not a stretched stale tile.
magick "$scratch/initial.png" -crop "$page_crop" +repage "$scratch/a.png"
magick "$scratch/back.png" -crop "$page_crop" +repage "$scratch/b.png"
magick compare -metric AE "$scratch/a.png" "$scratch/b.png" null: 2>"$scratch/difference"
echo 'PASS: pan/link history restores exact visible pixels'
zoom 1600
command f
wtype -s 150 'Distant needle' -s 150 -k Return -s 150
sleep 1
# A known coloured/text region, not an empty white portion of the huge page.
rm -f "$scratch/match.json"
swaymsg "exec /usr/bin/python3 '$geometry' '$scratch/match.json' '1 / 1 matches' 'label' > '$scratch/match.log' 2>&1" >/dev/null
for _ in {1..100}; do [[ -f "$scratch/match.json" ]] && break; sleep .1; done
if [[ ! -f "$scratch/match.json" ]]; then cat "$scratch/match.log" >&2; exit 1; fi
capture maximum-zoom
key Escape
capture maximum-zoom-page
zoom 400
command f
command a
wtype -s 150 'Distant needle' -s 150 -k Return -s 150
sleep 1
capture search
key Escape
key 0
expect_zoom 'Fit page'
capture fit-page
echo 'PASS: huge native page, cross-tile selection, internal link, pan, 1600% zoom and search'
