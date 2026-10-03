#!/usr/bin/env bash
set -euo pipefail

# Disposable native Sway session at 1280×900, scale 1, with a GTK chooser.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
cp "$scratch/outline.pdf" "$scratch/second résumé.pdf"
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
chooser=$(realpath tests/gtk-chooser.py)
state="$scratch/state/review/state.json"
title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
expect_title() {
    for _ in {1..100}; do
        if [[ $(title) == "$1" ]]; then echo "PASS: $1"; sleep .4; return; fi
        sleep .1
    done
    echo "Expected $1; got $(title)" >&2; exit 1
}
expect_state() {
    for _ in {1..50}; do
        if [[ -f $state ]] && jq -e "$1" "$state" >/dev/null; then return; fi
        sleep .1
    done
    echo "Failed state assertion: $1" >&2; cat "$state" >&2; exit 1
}
launch() {
    local argument=""
    if [[ -n ${1:-} ]]; then argument="'$scratch/$1'"; fi
    swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland GDK_BACKEND=wayland '$binary' $argument" >/dev/null
}
key() { wtype -s 150 -k "$1" -s 150; }
command() { wtype -s 150 -M ctrl -k "$1" -m ctrl -s 150; }
choose() {
    command o
    for _ in {1..100}; do
        if swaymsg -t get_tree | jq -e '.. | objects | select(.name? == "Open PDF" or .app_id? == "zenity")' >/dev/null; then break; fi
        sleep .1
    done
    rm -f "$scratch/chooser-done"
    swaymsg "exec /usr/bin/python3 '$chooser' '$scratch/$1' '$scratch/chooser-done' > '$scratch/chooser.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        if [[ -f $scratch/chooser-done ]]; then sleep .4; return; fi
        sleep .1
    done
    cat "$scratch/chooser.log" >&2; exit 1
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep .5
        grim "$REVIEW_SCREENSHOTS/tabs-$1.png"
    fi
}
quit() {
    command q
    for _ in {1..100}; do
        if [[ -z $(title) ]]; then return; fi
        sleep .1
    done
    echo 'Window did not close' >&2; exit 1
}

launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
key Right
wtype -s 150 -M ctrl -k l -m ctrl -s 150 '600' -s 150 -k Return -s 150
"$scratch/pointer" click 79 55
"$scratch/pointer" drag 240 450 327 450
"$scratch/pointer" scroll 800 450 317
expect_title 'Review — outline.pdf — 2/2 — 600%'
choose 'second résumé.pdf'
expect_title 'Review — second résumé.pdf — 1/2 — Fit page'
key F9
key 1
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
command Tab
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state '.sidebar.open and .sidebar.width == 327 and .recent[1].reading.scroll == [0,37.625]'
capture independent-reading
wtype -s 150 -M ctrl -M shift -k Tab -m shift -m ctrl -s 150
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
expect_state '.sidebar.open == false'
"$scratch/pointer" click 45 12
capture session-preference
# Two recent rows, Clear history, then the opt-in checkbox.
"$scratch/pointer" click 105 117
expect_state '.restore_session and (.session.files | length) == 2 and .session.active == 1'
"$scratch/pointer" click 800 450
quit
launch
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
expect_state '(.session.files | length) == 2 and .session.active == 1'
capture restored-session
command Tab
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state '.sidebar.open and .sidebar.width == 327'
echo 'PASS: independent tab reading/sidebar state, switching both ways, opt-in session restart and lazy restore'
choose outline.pdf
expect_title 'Review — outline.pdf — 2/2 — 600%'
expect_state '(.session.files | length) == 2'
command w
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
expect_state '(.session.files | length) == 1'
choose locked.pdf
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
wtype -s 150 'open-secret' -s 150 -k Return -s 150
expect_title 'Review — locked.pdf — 1/2 — Fit page'
key Right
key 1
key equal
expect_title 'Review — locked.pdf — 2/2 — 125%'
quit
launch
expect_title Review
capture encrypted-session-prompt
wtype -s 150 'wrong-secret' -s 150 -k Return -s 150
expect_title Review
expect_state '.session.files[1].reading.page == 1 and .session.files[1].reading.zoom.Percent == 1.25'
key Escape
command Tab
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
command Tab
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
wtype -s 150 'open-secret' -s 150 -k Return -s 150
expect_title 'Review — locked.pdf — 2/2 — 125%'
! grep -Eq 'open-secret|wrong-secret|password|Chapter one|Last alpha' "$state"
command w
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
command w
expect_title Review
expect_state '(.session.files | length) == 0'
quit
launch
expect_title Review
echo 'PASS: duplicate focus, tab close, encrypted restart/retry/cancel, no credentials/contents on disk and empty session restart'
quit

# A hand-authored over-limit session must be bounded before opening documents.
for index in {0..17}; do cp "$scratch/outline.pdf" "$scratch/tab$index.pdf"; done
jq --arg root "$scratch" '.restore_session = true | .session = {
    files: [range(0;18) | {path: ($root + "/tab" + tostring + ".pdf"),
        reading: {page:0,scroll:[0,0],zoom:"FitPage"},
        sidebar: {open:false,width:240,pages:false}}], active:17}' "$state" > "$state.new"
mv "$state.new" "$state"
launch
expect_title 'Review — tab0.pdf — 1/2 — Fit page'
expect_state '(.session.files | length) == 16 and .session.active == 0'
wtype -s 150 -M ctrl -M shift -k Tab -m shift -m ctrl -s 150
expect_title 'Review — tab15.pdf — 1/2 — Fit page'
capture bounded-session-overflow
command w
expect_title 'Review — tab14.pdf — 1/2 — Fit page'
expect_state '(.session.files | length) == 15'
echo 'PASS: over-limit session is bounded to 16, tab strip scrolls to selected document and last-tab close selects neighbour'
