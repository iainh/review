#!/usr/bin/env bash
set -euo pipefail

# Disposable native Sway session at 1280×900, scale 1, with a GTK chooser.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
cleanup() {
    set +e
    swaymsg '[app_id="^zenity$"] kill' >/dev/null
    # Sway's kill requests window close; it cannot bypass dirty confirmation.
    if [[ -x $scratch/pointer ]] && declare -F title >/dev/null && [[ -n $(title) ]]; then
        key Escape
        command q
        for _ in {1..16}; do
            [[ -z $(title) ]] && break
            "$scratch/pointer" click 570 479
        done
    fi
    swaymsg '[app_id="^review$"] kill' >/dev/null
    rm -rf "$scratch"
}
trap cleanup EXIT
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
"$scratch/pointer" click 79 153
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
expect_state '.sidebar.open and .sidebar.pages and .sidebar.width == 327 and .recent[1].reading.scroll == [-2,37.625]'
capture independent-reading
wtype -s 150 -M ctrl -M shift -k Tab -m shift -m ctrl -s 150
expect_title 'Review — second résumé.pdf — 1/2 — 100%'
expect_state '.sidebar.open == false'
"$scratch/pointer" click 45 66
capture session-preference
# Two recent rows, Clear history, then the opt-in checkbox.
"$scratch/pointer" click 105 171
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
quit

# Restore distinct non-default layouts and rotations, then switch live views.
jq --arg first "$scratch/outline.pdf" --arg second "$scratch/second résumé.pdf" '
    .session = {files: [
        {path:$first, reading:{page:0,scroll:[0,0],zoom:{Percent:1.25},
            layout:"Facing",rotation:"Counterclockwise"},
            sidebar:{open:false,width:240,pages:false}},
        {path:$second, reading:{page:0,scroll:[0,0],zoom:{Percent:1.5},
            layout:"Continuous",rotation:"Clockwise"},
            sidebar:{open:true,width:311,pages:true}}],active:0}' "$state" > "$state.new"
mv "$state.new" "$state"
launch
expect_title 'Review — outline.pdf — 1/2 — 125%'
expect_state '.session.active == 0 and .session.files[0].reading.layout == "Facing" and .session.files[0].reading.rotation == "Counterclockwise"'
capture restored-facing-rotation
command Tab
expect_title 'Review — second résumé.pdf — 1/2 — 150%'
expect_state '.session.active == 1 and .session.files[1].reading.layout == "Continuous" and .session.files[1].reading.rotation == "Clockwise"'
capture restored-continuous-rotation
command Tab
expect_title 'Review — outline.pdf — 1/2 — 125%'
expect_state '.session.active == 0 and .session.files[0].reading.layout == "Facing" and .session.files[0].reading.rotation == "Counterclockwise"'
echo 'PASS: per-tab facing/continuous layouts and asymmetric rotations restore and survive switching'
quit

# Edits are never stored in session metadata. Close prompts target the selected
# document and window exit walks every dirty tab without dropping buffers early.
REVIEW_FIXTURE_DIR="$scratch" cargo test export_annotation_fixture -- --ignored
cp "$scratch/annotations.pdf" "$scratch/second-annotations.pdf"
edit_note() {
    # The desktop titlebar (32) and always-visible document tab (22) precede
    # the existing library/viewer controls for both one and several tabs.
    local offset=54 text=$1
    "$scratch/pointer" click 48 "$((56 + offset))"
    "$scratch/pointer" click 1090 "$((205 + offset))"
    "$scratch/pointer" click 1140 "$((282 + offset))"
    wtype -s 150 -M ctrl -k a -m ctrl -s 150 "$text" -s 150
    "$scratch/pointer" click 1073 "$((355 + offset))"
}
launch annotations.pdf
expect_title 'Review — annotations.pdf — 1/2 — Fit page'
edit_note 'first unsaved tab'
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
choose second-annotations.pdf
expect_title 'Review — second-annotations.pdf — 1/2 — Fit page'
edit_note 'second unsaved tab'
expect_title 'Review — second-annotations.pdf * — 1/2 — Fit page'
capture dirty-two-documents
command Tab
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
chmod 400 "$scratch/annotations.pdf"
command w
capture dirty-tab-close
# Save fails against the read-only destination and retains this close target.
"$scratch/pointer" click 447 479
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
capture dirty-save-failed
key Escape
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
chmod 600 "$scratch/annotations.pdf"
command Tab
expect_title 'Review — second-annotations.pdf * — 1/2 — Fit page'
command q
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
"$scratch/pointer" click 570 479 # Discard advances, without dropping this buffer.
expect_title 'Review — second-annotations.pdf * — 1/2 — Fit page'
capture dirty-window-queue
key Escape
command Tab
expect_title 'Review — annotations.pdf * — 1/2 — Fit page'
echo 'PASS: dirty window-exit queue cancellation retains every tab and discarded buffer'
command w
"$scratch/pointer" click 447 479 # Save succeeds, then closes only this tab.
expect_title 'Review — second-annotations.pdf * — 1/2 — Fit page'
expect_state '(.session.files | length) == 1 and (.session.files[0].path | endswith("second-annotations.pdf"))'
command q
"$scratch/pointer" click 570 479 # Discard the final dirty tab, then exit.
for _ in {1..100}; do [[ -z $(title) ]] && break; sleep .1; done
[[ -z $(title) ]]
launch
expect_title 'Review — second-annotations.pdf — 1/2 — Fit page'
! grep -Eq 'first unsaved tab|second unsaved tab' "$state"
echo 'PASS: failed-save retention, save-and-close identity, final dirty exit and metadata-only restart'
quit
launch annotations.pdf
expect_title 'Review — annotations.pdf — 1/2 — Fit page'
"$scratch/pointer" click 48 110
capture dirty-saved-source
quit
REVIEW_FIXTURE_DIR="$scratch" cargo test verify_native_tab_close_output -- --ignored
echo 'PASS: reopened first tab contains saved edit; discarded second tab retains its original note'
