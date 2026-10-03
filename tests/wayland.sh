#!/usr/bin/env bash
set -euo pipefail

# Run in a disposable Sway session at 1280×900, scale 1.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'swaymsg "[title=\"^Review —\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
pdf=${REVIEW_TEST_PDF:-$scratch/handbook.pdf}
if [[ ! -f "$pdf" ]]; then
    curl -fsSL 'https://assets.ctfassets.net/2ntc334xpx65/2yRtkzYHiiBLLSguFsnQs9/419405cee8bd0a7b8f70e20cef22c190/The-openid-connect-handbook-v1.pdf' -o "$pdf"
fi
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
pdf=$(realpath "$pdf")
swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$pdf'" >/dev/null

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review —")) | .name'
}
expect_page() {
    for _ in {1..50}; do
        if [[ $(title) == *" — $1/${2:-45} — "* ]]; then
            echo "PASS: page $1"
            return
        fi
        sleep 0.1
    done
    echo "Expected page $1; got: $(title)" >&2
    exit 1
}
expect_zoom() {
    for _ in {1..50}; do
        if [[ $(title) == *" — $1" ]]; then
            echo "PASS: zoom $1"
            return
        fi
        sleep 0.1
    done
    echo "Expected zoom $1; got: $(title)" >&2
    exit 1
}
goto_page() {
    # Give the client time to bind the temporary virtual keyboard before input,
    # and to receive its final key before wtype destroys the device.
    wtype -s 150 -M ctrl -k g -m ctrl -s 150 "$1" -s 150 -k Return -s 150
}
key() {
    wtype -s 150 -k "$1" -s 150
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        # Title updates precede GPU presentation; also let panel animations settle.
        sleep 0.4
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}

expect_page 1
[[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review —")) | .shell') == xdg_shell ]]
echo 'PASS: native Wayland xdg_shell (not XWayland)'
goto_page 17
expect_page 17
capture goto-page
for invalid in 0 46 abc; do
    goto_page "$invalid"
    expect_page 17
done
capture invalid-page
wtype -s 150 -M ctrl -k g -m ctrl -s 150 'q' -s 150 -k Escape -s 150
expect_page 17
wtype -s 150 -M ctrl -k g -m ctrl -s 150 '23' -s 150
"$scratch/pointer" click 340 34
expect_page 23
"$scratch/pointer" click 165 34
expect_page 24
"$scratch/pointer" click 110 34
expect_page 23
key 1
expect_zoom '100%'
key equal
expect_zoom '125%'
capture zoomed-page
key 0
expect_zoom 'Fit page'
key 2
expect_zoom 'Fit width'
capture fit-width
wtype -s 150 -M ctrl -k l -m ctrl -s 150 '137.5' -s 150 -k Return -s 150
expect_zoom '138%'
capture explicit-zoom
wtype -s 150 -M ctrl -k l -m ctrl -s 150 'NaN' -s 150 -k Return -s 150
expect_zoom '138%'
capture invalid-zoom
key 0
goto_page 45
expect_page 45
key Next
expect_page 45
goto_page 1
expect_page 1
key Prior
expect_page 1
key Right
expect_page 2
key Left
expect_page 1
echo 'PASS: Wayland go-to-page and navigation'

goto_page 7
wtype -s 150 -M ctrl -k f -m ctrl -s 150 'rEcAp' -s 150 -k Return -s 150
sleep 2
expect_page 7
capture search-highlight
key F3
expect_page 17
wtype -s 150 -M shift -k Return -m shift -s 150
expect_page 7
key Return
expect_page 17
wtype -s 150 -M shift -k F3 -m shift -s 150
expect_page 7
key Return
expect_page 17
capture search-multiple-matches
key F3
expect_page 17
key F3
expect_page 44
key F3
expect_page 2
wtype -s 150 -M shift -k F3 -m shift -s 150
expect_page 44
wtype -s 150 -M ctrl -k f -m ctrl -s 150 'no-such-text-xyz' -s 150 -k Return -s 150
sleep 2
expect_page 44
capture search-no-matches
key Escape
expect_page 44
key Right
expect_page 45
echo 'PASS: Wayland search, repeat Enter, wraparound, no matches, and Escape'

goto_page 1
capture sidebar-no-outline
"$scratch/pointer" click 79 77
capture sidebar-previews
"$scratch/pointer" click 110 388
expect_page 2
goto_page 45
expect_page 45
capture sidebar-last-page
goto_page 1
"$scratch/pointer" scroll 110 400 700
capture sidebar-scrolled
"$scratch/pointer" click 110 388
# 700px scroll + the second visible row targets page 5, not page 2.
expect_page 5
"$scratch/pointer" drag 240 450 340 450
capture sidebar-resized
key F9
capture sidebar-hidden
key F9
expect_page 5
echo 'PASS: Wayland preview navigation, scrolling, resizing, and sidebar toggle'

# The handbook has no embedded outline. Exercise hierarchy and destinations
# with the synthetic two-page fixture instead.
swaymsg '[title="^Review —"] kill' >/dev/null
swaymsg "exec env -u DISPLAY XDG_STATE_HOME='$scratch/state' WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/outline.pdf'" >/dev/null
expect_page 1 2
# Sidebar preferences now follow the reader between documents.
# Window titles precede first-frame presentation with background rendering.
sleep 0.4
"$scratch/pointer" click 30 77
# Let the outline tab replace the thumbnail view before clicking a destination.
sleep 0.4
capture sidebar-nested-outline
"$scratch/pointer" click 100 128
expect_page 2 2
"$scratch/pointer" click 100 105
expect_page 1 2
"$scratch/pointer" click 13 105
capture sidebar-outline-collapsed
"$scratch/pointer" click 100 128
expect_page 1 2
"$scratch/pointer" click 13 105
"$scratch/pointer" click 100 128
expect_page 2 2
echo 'PASS: Wayland nested outline, collapse/expand, and destination navigation'
