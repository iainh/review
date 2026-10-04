#!/usr/bin/env bash
set -euo pipefail

# Disposable native Sway session at 1280×900, scale 1 (tests/sway.conf).
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
state="$scratch/state/review/state.json"
launch() {
    local argument=""
    if [[ -n ${1:-} ]]; then argument="'$scratch/$1'"; fi
    swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' $argument" >/dev/null
}
close() {
    swaymsg '[app_id="^review$"] kill' >/dev/null
    for _ in {1..50}; do
        if [[ -z $(title) ]]; then return; fi
        sleep .1
    done
    echo 'Viewer did not close' >&2; exit 1
}
title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
expect_title() {
    for _ in {1..100}; do
        if [[ $(title) == "$1" ]]; then echo "PASS: $1"; sleep .3; return; fi
        sleep .1
    done
    echo "Expected $1; got $(title)" >&2; exit 1
}
expect_state() {
    for _ in {1..50}; do
        if [[ -f $state ]] && jq -e "$1" "$state" >/dev/null; then return; fi
        sleep .1
    done
    echo "Failed state assertion: $1" >&2
    cat "$state" >&2; exit 1
}
key() { wtype -s 150 -k "$1" -s 150; }
command() { wtype -s 150 -M ctrl -k "$1" -m ctrl -s 150; }
bookmark() { wtype -s 150 -M ctrl -k b -m ctrl -s 150; }
open_menu() {
    local rights=$1
    command F2
    for ((i = 0; i < rights; i++)); do key Right; done
    key Return
}
menu_action() {
    local rights=$1 downs=$2
    open_menu "$rights"
    for ((i = 0; i < downs; i++)); do key Down; done
    key Return
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep .5
        grim "$REVIEW_SCREENSHOTS/persistence-$1.png"
    fi
}

launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
key Right
wtype -s 150 -M ctrl -k l -m ctrl -s 150 '600' -s 150 -k Return -s 150
expect_title 'Review — outline.pdf — 2/2 — 600%'
"$scratch/pointer" click 79 99
"$scratch/pointer" drag 240 450 327 450
"$scratch/pointer" scroll 800 450 317
bookmark
# At 600%, one PDF point is 8 logical pixels. Subtract the 16px page margin.
# The viewport also begins 16px before the page horizontally: retain -2pt.
expect_state '.recent[0].reading.page == 1 and .recent[0].reading.zoom.Percent == 6 and .recent[0].reading.scroll == [-2,37.625] and .recent[0].reading.layout == "Single" and .recent[0].reading.rotation == "None" and .sidebar.pages and .sidebar.width == 327 and (.bookmarks | length) == 1'
jq '.recent[0].reading' "$state" > "$scratch/reading.json"
capture saved-reading
close
launch outline.pdf
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state ".recent[0].reading == $(cat "$scratch/reading.json") and .sidebar.pages and .sidebar.width == 327"
capture restored-reading
key 2
expect_title 'Review — outline.pdf — 2/2 — Fit width'
close
launch outline.pdf
expect_title 'Review — outline.pdf — 2/2 — Fit width'
menu_action 2 1
expect_title 'Review — outline.pdf — 2/2 — 600%'
"$scratch/pointer" click 800 450
key F9
expect_state '.sidebar.open == false'
close
launch outline.pdf
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state '.sidebar.open == false and .sidebar.width == 327'
capture restored-hidden-sidebar
key F9
wtype -s 150 -M ctrl -k g -m ctrl -s 150 '1' -s 150 -k Return -s 150
"$scratch/pointer" click 800 450
key 0
expect_title 'Review — outline.pdf — 1/2 — Fit page'
open_menu 2
capture personal-bookmarks
key Down
key Return
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state ".recent[0].reading == $(cat "$scratch/reading.json")"
echo 'PASS: restart restores page, zoom, scroll, sidebar visibility/width/tab; bookmark restores its location'
close
launch
expect_title Review
open_menu 1
capture recent-files
key Return
expect_title 'Review — outline.pdf — 2/2 — 600%'
menu_action 1 1
capture clear-history-confirmation
key Return
expect_state '(.recent | length) == 0 and (.bookmarks | length) == 1'
key Left
expect_state '(.recent | length) == 0'
close
launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
bookmark
expect_state '(.bookmarks | length) == 2'
bookmark
expect_state '(.bookmarks | length) == 1'
echo 'PASS: recent-file open, clear-history confirmation, no silent history re-add, fresh reopen, bookmark add/remove'
close

# Bookmark and recent encrypted opens must authenticate again before restoration.
launch locked.pdf
expect_title Review
wtype -s 150 'open-secret' -s 150 -k Return -s 150
expect_title 'Review — locked.pdf — 1/2 — Fit page'
key Right
key 1
key equal
bookmark
expect_title 'Review — locked.pdf — 2/2 — 125%'
close
launch
expect_title Review
"$scratch/pointer" click 45 12
"$scratch/pointer" click 70 36
expect_title Review
capture encrypted-recent-prompt
wtype -s 150 'wrong-secret' -s 150 -k Return -s 150
expect_title Review
expect_state '.recent[0].reading.page == 1 and .recent[0].reading.zoom.Percent == 1.25'
key Escape
expect_title Review
"$scratch/pointer" click 156 12
"$scratch/pointer" click 175 85
expect_title Review
capture encrypted-bookmark-prompt
wtype -s 150 'open-secret' -s 150 -k Return -s 150
expect_title 'Review — locked.pdf — 2/2 — 125%'
! grep -Eq 'open-secret|wrong-secret|password|Chapter one|Last alpha' "$state"
echo 'PASS: encrypted recent/bookmark opens prompt; wrong password/cancel preserve saved location; authentication restores; no passwords/text on disk'
close

# Use floating mode to verify the client's saved normal window size, not tiling.
swaymsg 'for_window [app_id="^review$"] floating enable' >/dev/null
launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
swaymsg '[app_id="^review$"] floating enable, resize set 1037 px 713 px' >/dev/null
expect_state '.window.size == [1037,713]'
close
launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
[[ $(swaymsg -t get_tree | jq -c '.. | objects | select(.app_id? == "review") | [.rect.width,.rect.height]') == '[1037,713]' ]]
echo 'PASS: native normal window size restored across restart (Wayland owns placement)'
close
swaymsg 'for_window [app_id="^review$"] floating disable' >/dev/null
printf 'corrupt state\n' > "$state"
launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
expect_state '.recent[0].reading.page == 0 and .recent[0].reading.zoom == "FitPage"'
capture corrupt-state-recovered
close
echo 'PASS: corrupt state recovers to defaults and is replaced by valid JSON'
