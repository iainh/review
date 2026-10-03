#!/usr/bin/env bash
set -euo pipefail

# Run in the disposable 1280×900 scale-1 Sway session from tests/sway.conf.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_selection_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
geometry=$(realpath tests/pdf-page-geometry.py)

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'
}
open_pdf() {
    if [[ -n $(title) ]]; then
        swaymsg '[app_id="^review$"] kill' >/dev/null
    fi
    for _ in {1..100}; do
        [[ -z $(title) ]] && break
        sleep .1
    done
    [[ -z $(title) ]]
    swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/$1.pdf'" >/dev/null
    for _ in {1..100}; do
        [[ $(title) == "Review — $1.pdf — 1/2 — Fit page" ]] && break
        sleep .1
    done
    [[ $(title) == "Review — $1.pdf — 1/2 — Fit page" ]]
    sleep .7
    page_geometry
}
page_geometry() {
    rm -f "$scratch/page.json"
    swaymsg "exec /usr/bin/python3 '$geometry' '$scratch/page.json' 'PDF page 1' > '$scratch/geometry.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        [[ -f "$scratch/page.json" ]] && return
        sleep .1
    done
    cat "$scratch/geometry.log" >&2
    exit 1
}
point() {
    # The fixture MediaBox is 300×400 PDF points with a nonzero origin.
    /usr/bin/python3 - "$scratch/page.json" "$1" "$2" <<'PY'
import json, sys
x, y, width, height = json.load(open(sys.argv[1]))
print(round(x + float(sys.argv[2]) * width / 300),
      round(y + float(sys.argv[3]) * height / 400))
PY
}
pointer() {
    local action=$1 x y end_x end_y
    read -r x y < <(point "$2" "$3")
    if [[ $# == 5 ]]; then
        read -r end_x end_y < <(point "$4" "$5")
        "$scratch/pointer" "$action" "$x" "$y" "$end_x" "$end_y"
    elif [[ ${4:-} == copy ]]; then
        "$scratch/pointer" "$action" "$x" "$y" "$((x + 24))" "$((y + 17))"
    else
        "$scratch/pointer" "$action" "$x" "$y"
    fi
}
command_key() { wtype -s 150 -M ctrl -k "$1" -m ctrl -s 150; }
copy() { command_key c; }
expect_clipboard() {
    local text
    for _ in {1..30}; do
        text=$(wl-paste --no-newline 2>/dev/null || true)
        if [[ "$text" == "$1" ]]; then
            printf 'PASS: clipboard %q\n' "$text"
            return
        fi
        sleep .1
    done
    printf 'Expected clipboard %q; got %q\n' "$1" "$text" >&2
    exit 1
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep .4
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}

open_pdf selection
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
command_key a
copy
expect_clipboard $'Left first\nLeft second\n\nRight first\nRight second\n\nRotated'
capture selection-all
pointer drag 31 64 82 64
copy
expect_clipboard 'Left firs'
pointer drag 82 64 31 64
copy
expect_clipboard 'Left firs'
capture selection-drag
pointer double 43 64
copy
expect_clipboard Left
pointer triple 43 64
copy
expect_clipboard $'Left first\nLeft second'
capture selection-paragraph
pointer double 180 64
# Physical keyboards persist. Keep a virtual keyboard alive during mouse-only
# copy too: Wayland clipboard ownership needs a live keyboard focus serial.
swaymsg 'exec wtype -s 150 -M shift -m shift -s 5000' >/dev/null
sleep .3
pointer right 180 64 copy
expect_clipboard Right
pointer right 180 64
capture selection-context
pointer drag 44 280 44 205
copy
expect_clipboard Rotated
capture selection-rotated
wtype -s 150 -k 1 -s 150
copy
expect_clipboard Rotated
capture selection-zoom
wtype -s 150 -k Right -s 150
command_key a
copy
expect_clipboard 'Last alpha'

# Keep one virtual keyboard through each edit, matching a physical keyboard.
# Removing it between typing and copying sends focus loss to the client.
wtype -s 150 -M ctrl -k g -m ctrl -s 150 '42' -s 150 -M ctrl -k a -k c -m ctrl -s 150
expect_clipboard 42
wtype -s 150 -k Escape -s 150
wtype -s 150 -M ctrl -k l -m ctrl -s 150 '137.5' -s 150 -M ctrl -k a -k c -m ctrl -s 150
expect_clipboard 137.5
wtype -s 150 -k Escape -s 150
wtype -s 150 -M ctrl -k f -m ctrl -s 150 'needle only' -s 150 -M ctrl -k a -k c -m ctrl -s 150
expect_clipboard 'needle only'
echo 'PASS: page, zoom and search fields retain Select All and Copy'

open_pdf unicode
command_key a
copy
expect_clipboard $'e\u0301 中文 אב'
echo 'PASS: native clipboard preserves combining marks, Chinese and Hebrew ActualText'

open_pdf restricted
wl-copy 'permission sentinel'
command_key a
copy
expect_clipboard 'permission sentinel'
pointer double 43 64
copy
expect_clipboard 'permission sentinel'
pointer right 43 64
capture selection-denied
expect_clipboard 'permission sentinel'
echo 'PASS: copy-restricted PDF blocks keyboard, pointer selection and context-menu copy'
