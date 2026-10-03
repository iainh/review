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
"$scratch/pointer" drag 513 180 619 180
copy
expect_clipboard 'Left firs'
"$scratch/pointer" drag 619 180 513 180
copy
expect_clipboard 'Left firs'
capture selection-drag
"$scratch/pointer" double 540 180
copy
expect_clipboard Left
"$scratch/pointer" triple 540 180
copy
expect_clipboard $'Left first\nLeft second'
capture selection-paragraph
"$scratch/pointer" double 836 180
# Physical keyboards persist. Keep a virtual keyboard alive during mouse-only
# copy too: Wayland clipboard ownership needs a live keyboard focus serial.
swaymsg 'exec wtype -s 150 -M shift -m shift -s 5000' >/dev/null
sleep .3
"$scratch/pointer" right 836 180 860 197
expect_clipboard Right
"$scratch/pointer" right 836 180
capture selection-context
"$scratch/pointer" drag 542 603 542 490
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
"$scratch/pointer" double 540 200
copy
expect_clipboard 'permission sentinel'
"$scratch/pointer" right 540 200
capture selection-denied
expect_clipboard 'permission sentinel'
echo 'PASS: copy-restricted PDF blocks keyboard, pointer selection and context-menu copy'
