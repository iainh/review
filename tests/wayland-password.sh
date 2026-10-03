#!/usr/bin/env bash
set -euo pipefail

# Same disposable native Wayland/GTK session as tests/wayland-open.sh.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; swaymsg "[title=\"^Open PDF$\"] kill" >/dev/null; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
chooser=$(realpath tests/gtk-chooser.py)

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'
}
expect_title() {
    for _ in {1..100}; do
        if [[ $(title) == "$1" ]]; then
            echo "PASS: $1"
            return
        fi
        sleep 0.1
    done
    echo "Expected: $1; got: $(title)" >&2
    exit 1
}
key() { wtype -s 150 -k "$1" -s 150; }
dialog_visible() {
    swaymsg -t get_tree | jq -e '.. | objects | select(.name? == "Open PDF" or .app_id? == "zenity")' >/dev/null
}
choose() {
    swaymsg '[app_id="^review$"] focus' >/dev/null
    wtype -s 150 -M ctrl -k o -m ctrl -s 150
    for _ in {1..100}; do
        if dialog_visible; then break; fi
        sleep 0.1
    done
    dialog_visible
    local id
    id=$(swaymsg -t get_tree | jq -r '.. | objects | select(.name? == "Open PDF" or .app_id? == "zenity") | .id')
    swaymsg "[con_id=$id] focus" >/dev/null
    rm -f "$scratch/chooser-done"
    swaymsg "exec /usr/bin/python3 '$chooser' '$scratch/$1' '$scratch/chooser-done' > '$scratch/chooser.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        [[ -f "$scratch/chooser-done" ]] && break
        sleep 0.1
    done
    if [[ ! -f "$scratch/chooser-done" ]]; then cat "$scratch/chooser.log" >&2; exit 1; fi
    for _ in {1..100}; do
        if ! dialog_visible; then break; fi
        sleep 0.1
    done
    ! dialog_visible
    sleep 0.5
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep 0.5
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}
launch() {
    swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/$1'" >/dev/null
}

# Startup authentication, incorrect password, and Escape without a prior PDF.
launch locked.pdf
expect_title Review
sleep 0.5
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
echo 'PASS: native Wayland xdg_shell (not XWayland)'
capture password-startup
wtype -s 150 'wrong-q' -s 150
capture password-startup-masked
key Return
capture password-startup-retry
key Escape
expect_title Review
capture password-cancel-empty
swaymsg '[app_id="^review$"] kill' >/dev/null

# Cancellation and retry must preserve a non-default page and zoom.
launch outline.pdf
expect_title 'Review — outline.pdf — 1/2 — Fit page'
key Right
key 1
key equal
preserved='Review — outline.pdf — 2/2 — 125%'
expect_title "$preserved"
choose locked.pdf
expect_title "$preserved"
wtype -s 150 'wrong-q' -s 150
key Right
key equal
wtype -s 150 -M ctrl -k o -m ctrl -s 150
! dialog_visible
expect_title "$preserved"
capture password-masked
key Return
capture password-retry
expect_title "$preserved"
wtype -s 150 -M ctrl -k z -m ctrl -s 150
capture password-retry-no-undo
"$scratch/pointer" click 165 12
expect_title "$preserved"
"$scratch/pointer" click 558 523
expect_title "$preserved"
capture password-cancel-preserved
# This navigation proves the modal was actually dismissed by Cancel.
key Left
expect_title 'Review — outline.pdf — 1/2 — 125%'
key Right
choose locked.pdf
wtype -s 150 'wrong-again' -s 150 -k Return -s 150
sleep 0.3
wtype -s 150 'open-secret' -s 150
"$scratch/pointer" click 493 523
expect_title 'Review — locked.pdf — 1/2 — Fit page'
capture password-unlocked-restricted
wtype -s 150 -M ctrl -k f -m ctrl -s 150 'Chapter' -s 150 -k Return -s 150
sleep 0.5
expect_title 'Review — locked.pdf — 1/2 — Fit page'
capture password-restricted-search
key Escape

# Every new open requires fresh credentials. Owner access removes restrictions.
choose outline.pdf
choose locked.pdf
expect_title 'Review — outline.pdf — 2/2 — 125%'
wtype -s 150 'owner-secret' -s 150 -k Return -s 150
expect_title 'Review — locked.pdf — 1/2 — Fit page'
capture password-owner-access
choose restricted.pdf
expect_title 'Review — restricted.pdf — 1/2 — Fit page'
capture password-empty-user-low-quality
echo 'PASS: startup password, masked entry, retry, Escape/Cancel, modal input isolation, preserved page/zoom, user/owner access, and empty-password permissions'
