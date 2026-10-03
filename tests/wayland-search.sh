#!/usr/bin/env bash
set -euo pipefail

# Disposable native 1280×900 scale-1 Sway session; see README.md.
: "${WAYLAND_DISPLAY:?Run in the disposable Wayland session}"
: "${SWAYSOCK:?Set SWAYSOCK to its Sway socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build --locked
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_search_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/pointer.xml"
wayland-scanner client-header "$scratch/pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/pointer.xml" "$scratch/pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)

title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
expect_page() {
    for _ in {1..100}; do
        if [[ $(title) == *" — $1/$2 — "* ]]; then
            echo "PASS: page $1/$2"
            return
        fi
        sleep .1
    done
    echo "Expected page $1/$2; got $(title)" >&2
    exit 1
}
open_pdf() {
    if [[ -n $(title) ]]; then swaymsg '[app_id="^review$"] kill' >/dev/null; fi
    for _ in {1..100}; do [[ -z $(title) ]] && break; sleep .1; done
    [[ -z $(title) ]]
    swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/$1.pdf'" >/dev/null
    expect_page 1 "$2"
    swaymsg '[app_id="^review$"] focus' >/dev/null
    sleep .7
}
key() { wtype -s 150 -k "$1" -s 150; }
query() { wtype -s 150 -M ctrl -k f -m ctrl -s 150 "$1" -s 150 -k Return -s 150; }
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep .2
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}

open_pdf search 2
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
echo 'PASS: native Wayland xdg_shell, not XWayland'
query alpha
sleep .3
capture search-results
# Three occurrences on page one, one on page two; wrap both directions.
key F3; expect_page 1 2
key F3; expect_page 1 2
key F3; expect_page 2 2
capture search-next-page
key F3; expect_page 1 2
wtype -s 150 -M shift -k F3 -m shift -s 150
expect_page 2 2
wtype -s 150 -M shift -k Return -m shift -s 150
expect_page 1 2
key Return; expect_page 2 2
echo 'PASS: Enter, Shift+Enter, F3, Shift+F3 and bidirectional wraparound'

# Click the first snippet to navigate directly from page two.
"$scratch/pointer" click 145 80
expect_page 1 2
# Case-sensitive alpha excludes Alpha; whole words also exclude alphabet.
"$scratch/pointer" click 276 57
sleep .3
key F3; expect_page 1 2
key F3; expect_page 2 2
"$scratch/pointer" click 382 57
sleep .3
key F3; expect_page 1 2
key F3; expect_page 2 2
capture search-options
query Alpha
sleep .3
expect_page 1 2
key F3; expect_page 1 2
echo 'PASS: snippet navigation and automatic case/whole-word rescanning'

# Restore case-insensitive matching, keeping whole-word mode for the phrase.
"$scratch/pointer" click 276 57
query 'needle phrase'
sleep .3
expect_page 1 2
capture search-multiline
query 'not-present-xyz'
sleep .3
capture search-no-matches
key F3; expect_page 1 2
key Escape
key Right; expect_page 2 2
echo 'PASS: multiline phrase, no matches and closing search clears navigation'

open_pdf restricted 2
wl-copy 'permission sentinel'
query Chapter
sleep .3
capture search-copy-restricted
wtype -s 150 -M ctrl -k a -k c -m ctrl -s 150
[[ $(wl-paste --no-newline) == Chapter ]]
key Escape
wtype -s 150 -M ctrl -k a -k c -m ctrl -s 150
[[ $(wl-paste --no-newline) == Chapter ]]
echo 'PASS: restricted PDF search works; query copy works; page copying remains denied'

open_pdf progress 1400
query alpha
capture search-scanning
key F3; key F3; key F3
expect_page 2 1400
capture search-scanning-navigated
# Supersede three hits per page with one per page; F3 must now change pages.
query alphabet
sleep .3
key F3
expect_page 3 1400
capture search-superseded
wtype -s 150 -M ctrl -k f -k BackSpace -m ctrl -s 150
sleep .5
key F3
expect_page 3 1400
capture search-cancelled
key Escape
key Right
expect_page 4 1400
echo 'PASS: live partial-result navigation, superseded query and blank-query cancellation'
