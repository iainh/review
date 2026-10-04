#!/usr/bin/env bash
set -euo pipefail

# Disposable 1280×900 scale-1 native Sway session, as in wayland-selection.sh.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
engine=$(command -v tesseract)
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_ocr_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
accessibility_client=$(realpath tests/ocr-accessibility.py)
geometry_client=$(realpath tests/pdf-page-geometry.py)
mkdir "$scratch/missing" "$scratch/delayed" "$scratch/normal" "$scratch/private"
chmod 700 "$scratch/private"
# Record the actual image directory and its mode; do not assume the runtime's
# secure temp-directory choice honours TMPDIR. Recognition uses real Tesseract.
cat > "$scratch/normal/tesseract" <<EOF
#!/bin/sh
if [ "\$1" != --list-langs ]; then
    /usr/bin/dirname "\$1" > '$scratch/engine.directory'
    /usr/bin/stat -c %a "\$(/usr/bin/dirname "\$1")" > '$scratch/engine.mode'
fi
exec '$engine' "\$@"
EOF
# This helper emits no recognized data. Language discovery uses the real engine;
# a cancellable sleeping process holds the progress state for native input.
cat > "$scratch/delayed/tesseract" <<EOF
#!/bin/sh
if [ "\$1" = --list-langs ]; then exec '$engine' "\$@"; fi
/usr/bin/dirname "\$1" > '$scratch/engine.directory'
/usr/bin/stat -c %a "\$(/usr/bin/dirname "\$1")" > '$scratch/engine.mode'
echo \$\$ > '$scratch/engine.pid'
exec /bin/sleep 30
EOF
chmod +x "$scratch/delayed/tesseract" "$scratch/normal/tesseract"

launch=0
title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
open_pdf() {
    if [[ -n $(title) ]]; then
        swaymsg '[app_id="^review$"] kill' >/dev/null
    fi
    for _ in {1..100}; do [[ -z $(title) ]] && break; sleep .1; done
    [[ -z $(title) ]]
    rm -f "$scratch/engine.pid" "$scratch/engine.directory" "$scratch/engine.mode"
    local path=${2:-$scratch/normal:$PATH}
    launch=$((launch + 1))
    swaymsg "exec /usr/bin/env -u DISPLAY XDG_STATE_HOME='$scratch/state-$launch' WINIT_UNIX_BACKEND=wayland PATH='$path' TMPDIR='$scratch/private' '$binary' '$scratch/$1.pdf'" >/dev/null
    for _ in {1..100}; do
        [[ $(title) == "Review — $1.pdf — 1/2 — Fit page" ]] && break
        sleep .1
    done
    [[ $(title) == "Review — $1.pdf — 1/2 — Fit page" ]]
    [[ $(swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .shell') == xdg_shell ]]
    accessibility status 'PDF page 1'
}
command_key() { wtype -s 100 -M ctrl -k "$1" -m ctrl -s 100; }
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}
accessibility() {
    rm -f "$scratch/a11y.json"
    swaymsg "exec /usr/bin/python3 '$accessibility_client' '$scratch/a11y.json' '$1' '$2' > '$scratch/a11y.log' 2>&1" >/dev/null
    for _ in {1..150}; do
        [[ -f "$scratch/a11y.json" ]] && return
        sleep .1
    done
    cat "$scratch/a11y.log" >&2
    exit 1
}
wait_text() { accessibility status "$1"; }
click_word() {
    local label
    case "$1" in
        ocr) accessibility click Tools; label=OCR ;;
        recognize) label='Recognize page 1' ;;
        find) label=Find ;;
        close) label='Close OCR' ;;
        cancel) label=Cancel ;;
        *) echo "Unknown control: $1" >&2; exit 1 ;;
    esac
    accessibility click "$label"
    sleep .3
}
page_pointer() {
    rm -f "$scratch/page.json"
    swaymsg "exec /usr/bin/python3 '$geometry_client' '$scratch/page.json' 'PDF page 1' > '$scratch/geometry.log' 2>&1" >/dev/null
    for _ in {1..100}; do [[ -f "$scratch/page.json" ]] && break; sleep .1; done
    [[ -f "$scratch/page.json" ]] || { cat "$scratch/geometry.log" >&2; exit 1; }
    local x y
    read -r x y < <(/usr/bin/python3 - "$scratch/page.json" "$2" "$3" <<'PY'
import json, sys
x, y, width, height = json.load(open(sys.argv[1]))
print(round(x + float(sys.argv[2]) * width / 300), round(y + float(sys.argv[3]) * height / 400))
PY
    )
    "$scratch/pointer" "$1" "$x" "$y"
}
copy_all() { command_key a; command_key c; }
expect_clipboard() {
    local text
    for _ in {1..30}; do
        text=$(wl-paste --no-newline 2>/dev/null || true)
        if [[ "$text" == "$1" ]]; then printf 'PASS: clipboard %q\n' "$text"; return; fi
        sleep .1
    done
    printf 'Expected clipboard %q; got %q\n' "$1" "$text" >&2
    exit 1
}
expect_private_clean() {
    if [[ -f "$scratch/engine.directory" ]]; then
        [[ $(cat "$scratch/engine.mode") == 700 ]]
        local directory
        directory=$(cat "$scratch/engine.directory")
        for _ in {1..30}; do [[ ! -d "$directory" ]] && break; sleep .1; done
        [[ ! -d "$directory" ]]
    fi
    for _ in {1..30}; do
        [[ -z $(ls -A "$scratch/private") ]] && return
        sleep .1
    done
    echo 'OCR temporary files were not cleaned' >&2
    exit 1
}

open_pdf scan
before=$(sha256sum "$scratch/scan.pdf")
wl-copy 'before recognition'
copy_all
expect_clipboard 'before recognition'
# Submit a search before recognition; completion must refresh its empty results.
wtype -s 150 -M ctrl -k f -m ctrl -s 150 amber -s 250
click_word find
wait_text nomatches
click_word ocr
wait_text ready
accessibility language Language
capture ocr-languages
accessibility choose eng
# Native accessibility actions do not generate an outside pointer press.
# Dismiss the selector before capturing the independent recognition state.
"$scratch/pointer" click 1200 300
click_word recognize
wait_text 'page1recognized'
expect_private_clean
wait_text '1/1matches'
capture ocr-search-refreshed
click_word close
page_pointer click 150 200
copy_all
# Paragraph segmentation can vary across engines. The independently known
# scanned words must still appear in order, without dropped or duplicated text.
text=$(wl-paste --no-newline)
[[ $(printf '%s' "$text" | tr '\n' ' ' | xargs) == 'Scanned amber fox Local violet river' ]]
echo 'PASS: actual OCR text is selectable/copyable and submitted search refreshed'
capture ocr-selection
[[ $(sha256sum "$scratch/scan.pdf") == "$before" ]]
page_pointer double 145 47
command_key c
expect_clipboard amber
capture ocr-word-selection
wtype -s 100 -k Right -s 200
wl-copy 'unrecognized second page'
copy_all
expect_clipboard 'unrecognized second page'
open_pdf scan
wl-copy 'new session'
copy_all
expect_clipboard 'new session'
echo 'PASS: recognition is per-page and session-only; original bytes unchanged'

open_pdf restricted-scan
wl-copy 'permission sentinel'
accessibility click Tools
accessibility click 'Page text'
accessibility pane ''
capture ocr-page-text-empty
click_word ocr
wait_text ready
click_word recognize
wait_text 'page1recognized'
expect_private_clean
accessibility pane 'Scanned amber fox Local violet river'
capture ocr-page-text-refreshed
accessibility click 'Close page text'
capture ocr-copy-denied
click_word close
"$scratch/pointer" click 500 400
copy_all
expect_clipboard 'permission sentinel'
"$scratch/pointer" right 500 400
capture ocr-context-denied
expect_clipboard 'permission sentinel'
"$scratch/pointer" click 1000 400
wtype -s 150 -M ctrl -k f -m ctrl -s 150 violet -s 250
click_word find
wait_text '1/1matches'
echo 'PASS: copy restrictions apply to OCR; search remains available'

open_pdf native
click_word ocr
wait_text ready
click_word recognize
wait_text alreadyhasnativetext
capture ocr-native-refused
expect_private_clean
"$scratch/pointer" click 500 400
copy_all
expect_clipboard 'Native original text'
echo 'PASS: native text is never replaced by OCR'

open_pdf blank
click_word ocr
wait_text ready
click_word recognize
wait_text notextrecognized
capture ocr-empty
expect_private_clean

open_pdf scan "$scratch/delayed"
click_word ocr
wait_text ready
click_word recognize
for _ in {1..100}; do [[ -f "$scratch/engine.pid" ]] && break; sleep .05; done
[[ -f "$scratch/engine.pid" ]]
pid=$(cat "$scratch/engine.pid")
[[ -d /proc/$pid ]]
[[ $(cat "$scratch/engine.mode") == 700 ]]
[[ -f $(cat "$scratch/engine.directory")/page.ppm ]]
capture ocr-progress
wtype -s 100 -k Right -k 1 -s 200
[[ $(title) == 'Review — scan.pdf — 2/2 — 100%' ]]
click_word cancel
wait_text ocrcancelled
for _ in {1..30}; do [[ ! -d /proc/$pid ]] && break; sleep .1; done
[[ ! -d /proc/$pid ]]
expect_private_clean
capture ocr-cancelled
wtype -s 100 -k Left -s 200
wl-copy 'cancelled sentinel'
copy_all
expect_clipboard 'cancelled sentinel'
echo 'PASS: responsive native navigation/zoom, cancellation, engine reaping and private cleanup'

open_pdf scan "$scratch/missing"
click_word ocr
wait_text localocrneedstesseract
capture ocr-missing-engine
expect_private_clean
echo 'PASS: missing-engine installation guidance without downloads'
