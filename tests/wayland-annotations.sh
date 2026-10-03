#!/usr/bin/env bash
set -euo pipefail

# Disposable native 1280×900 scale-1 Sway session; no PDF JavaScript or services.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_annotation_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
chooser=$(realpath tests/gtk-chooser.py)
title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
expect_title() {
    for _ in {1..100}; do
        if [[ $(title) == "$1" ]]; then printf 'PASS: %s\n' "$1"; return; fi
        sleep .1
    done
    printf 'Expected title %s; got %s\n' "$1" "$(title)" >&2
    exit 1
}
key() { wtype -s 150 -k "$1" -s 150; }
command_key() { wtype -s 150 -M ctrl -k "$1" -m ctrl -s 150; }
# Coordinates below are viewer-relative; the library toolbar adds 22 pixels.
click() { "$scratch/pointer" click "$1" "$(($2 + 22))"; }
# Page/sidebar content also follows the added reading-layout toolbar.
click_content() { "$scratch/pointer" click "$1" "$(($2 + 44))"; }
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep .4
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}
open_pdf() {
    if [[ -n $(title) ]]; then command_key q; expect_title ''; fi
    swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/$1.pdf'" >/dev/null
    expect_title "Review — $1.pdf — 1/2 — Fit page"
    sleep 1
}

open_pdf annotations
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
click 48 34
click_content 1090 161
click_content 1140 238
wtype -s 150 -M ctrl -k a -m ctrl -s 150 'Edited native note' -s 150
click_content 1073 311
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
command_key s
expect_title 'Review — annotations.pdf — 1/2 — Fit page'
click_content 1151 311
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
click 355 34
expect_title 'Review — annotations.pdf — 1/2 — Fit page'
click 400 34
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
click 355 34
expect_title 'Review — annotations.pdf — 1/2 — Fit page'
click 820 720
command_key a
click 124 34
sleep .5
command_key a
click 194 34
sleep .5
command_key a
click 283 34
sleep .5
capture annotations-markup
click_content 1097 98
click 850 500
sleep .5
click_content 1137 98
"$scratch/pointer" drag 590 712 830 787
sleep .5
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
command_key w
capture annotations-unsaved
"$scratch/pointer" click 627 479
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
key Escape
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
wtype -s 150 -M ctrl -M shift -k s -m shift -m ctrl -s 150
for _ in {1..100}; do
    swaymsg -t get_tree | jq -e '.. | objects | select(.app_id? == "zenity" or .name? == "Save PDF As")' >/dev/null && break
    sleep .1
done
swaymsg "exec /usr/bin/python3 '$chooser' '$scratch/native-save.pdf' '$scratch/chooser-done' > '$scratch/chooser.log' 2>&1" >/dev/null
for _ in {1..100}; do [[ -f "$scratch/chooser-done" ]] && break; sleep .1; done
if [[ ! -f "$scratch/chooser-done" ]]; then cat "$scratch/chooser.log" >&2; exit 1; fi
expect_title 'Review — native-save.pdf — 1/2 — Fit page'
for _ in {1..100}; do
    if jq -e --arg path "$scratch/native-save.pdf" '.recent[0].path == $path' "$scratch/state/review/state.json" >/dev/null 2>&1; then break; fi
    sleep .1
done
jq -e --arg path "$scratch/native-save.pdf" '.recent[0].path == $path' "$scratch/state/review/state.json" >/dev/null
echo 'PASS: Save As updates reading-state identity before reopening'
open_pdf native-save
click 48 34
click_content 92 56
sleep .8
capture annotations-saved
echo 'PASS: native editing, deletion, undo/redo, unsaved-close cancellation and Save As'

open_pdf annotate-only
wl-copy 'copy permission sentinel'
command_key a
command_key c
[[ $(wl-paste --no-newline) == 'copy permission sentinel' ]]
click 124 34
expect_title 'Review — annotate-only.pdf * — 1/2 — Fit page'
command_key s
expect_title 'Review — annotate-only.pdf — 1/2 — Fit page'
capture annotations-copy-denied
REVIEW_FIXTURE_DIR="$scratch" cargo test verify_native_annotation_output -- --ignored
echo 'PASS: reopened native PDFs preserve annotations, one quad per line, encrypted permissions and original Save As source'
open_pdf restricted
click 48 34
command_key a
click 124 34
expect_title 'Review — restricted.pdf — 1/2 — Fit page'
capture annotations-denied
echo 'PASS: annotation-denied PDF cannot create markup'
