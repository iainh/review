#!/usr/bin/env bash
set -euo pipefail

# Disposable Sway session, 1280×900 scale 1, as in tests/wayland.sh.
# Only internal links are activated. External URL requests are tested through
# egui platform output in Rust tests, never dispatched to a real browser.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'swaymsg "[title=\"^Review — links.pdf\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_links_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/links.pdf'" >/dev/null

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review — links.pdf")) | .name'
}
expect() {
    for _ in {1..100}; do
        if [[ $(title) == *" — $1/2 — $2" ]]; then
            echo "PASS: page $1, $2"
            sleep 0.5
            return
        fi
        sleep 0.1
    done
    echo "Expected page $1 at $2; got $(title)" >&2
    exit 1
}
capture() {
    sleep 0.4
    grim "$scratch/$1.png"
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        cp "$scratch/$1.png" "$REVIEW_SCREENSHOTS/links-$1.png"
    fi
}
back() { wtype -s 150 -M alt -k Left -m alt -s 150; }
forward() { wtype -s 150 -M alt -k Right -m alt -s 150; }
same_page_pixels() {
    # Exclude toolbar history-button state, compare the actual rendered page.
    magick "$scratch/$1.png" -crop 1040x875+240+25 +repage "$scratch/a.png"
    magick "$scratch/$2.png" -crop 1040x875+240+25 +repage "$scratch/b.png"
    magick compare -metric AE "$scratch/a.png" "$scratch/b.png" null: 2>"$scratch/difference"
    # ImageMagick exits zero only for identical pixels, ignoring PNG metadata.
    echo "PASS: identical page pixels for $1 and $2"
}

expect 1 'Fit page'
capture initial
"$scratch/pointer" move 690 294 2500 &
pointer_pid=$!
sleep 1.2
capture hover
wait "$pointer_pid"
"$scratch/pointer" right-click 630 282
capture copy-menu
# Dismiss without ever clicking a web/email link.
"$scratch/pointer" click 400 800

"$scratch/pointer" click 630 157
expect 2 '225%'
capture xyz
"$scratch/pointer" scroll 600 400 200
capture scrolled
back
expect 1 'Fit page'
capture back
same_page_pixels initial back
forward
expect 2 '225%'
capture forward-scrolled
same_page_pixels scrolled forward-scrolled
back
expect 1 'Fit page'
"$scratch/pointer" click 630 219
expect 2 '225%'
capture named
same_page_pixels xyz named
back
expect 1 'Fit page'
"$scratch/pointer" click 100 64
expect 2 '225%'
capture outline-xyz
same_page_pixels xyz outline-xyz
"$scratch/pointer" click 100 86
expect 2 'Fit width'
capture outline-fit-width
back
expect 2 '225%'
capture same-page-back
same_page_pixels xyz same-page-back
forward
expect 2 'Fit width'
capture same-page-forward
same_page_pixels outline-fit-width same-page-forward
back
expect 2 '225%'
back
expect 1 'Fit page'
"$scratch/pointer" click 630 655
expect 1 'Fit page'
echo 'PASS: blocked Launch action does not navigate'
echo 'PASS: native links, named/outline destinations, hover/menu, scroll and zoom history'
