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
binary=$(realpath target/debug/review)
pdf=$(realpath "$pdf")
swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland '$binary' '$pdf'" >/dev/null

title() {
    swaymsg -t get_tree | jq -r '.. | objects | select(.name? | strings | startswith("Review —")) | .name'
}
expect_page() {
    for _ in {1..50}; do
        if [[ $(title) == *" — $1/45 — "* ]]; then
            echo "PASS: page $1"
            return
        fi
        sleep 0.1
    done
    echo "Expected page $1; got: $(title)" >&2
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
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}

expect_page 1
goto_page 17
expect_page 17
capture goto-page
for invalid in 0 46 abc; do
    goto_page "$invalid"
    expect_page 17
done
capture invalid-page
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
