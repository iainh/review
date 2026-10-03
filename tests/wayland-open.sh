#!/usr/bin/env bash
set -euo pipefail

# Run in a disposable Sway session with a GTK file chooser and python3-pyatspi.
: "${WAYLAND_DISPLAY:?Run this test in a Wayland Sway session}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; swaymsg "[title=\"^Open PDF$\"] kill" >/dev/null; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
cp "$scratch/outline.pdf" "$scratch/résumé 日本語 document.pdf"
printf 'not a PDF\n' > "$scratch/broken.pdf"
binary=$(realpath target/debug/review)
chooser=$(realpath tests/gtk-chooser.py)
swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland GDK_BACKEND=wayland '$binary'" >/dev/null

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
wait_for_dialog_close() {
    for _ in {1..100}; do
        if ! dialog_visible; then
            sleep 0.3
            return
        fi
        sleep 0.1
    done
    echo 'Native file dialog did not close' >&2
    exit 1
}
open_dialog() {
    swaymsg '[app_id="^review$"] focus' >/dev/null
    wtype -s 150 -M ctrl -k o -m ctrl -s 150
    for _ in {1..100}; do
        if dialog_visible; then
            local id
            id=$(swaymsg -t get_tree | jq -r '.. | objects | select(.name? == "Open PDF" or .app_id? == "zenity") | .id')
            swaymsg "[con_id=$id] focus" >/dev/null
            echo 'PASS: native PDF file chooser'
            return
        fi
        sleep 0.1
    done
    echo 'Native PDF file chooser did not appear' >&2
    exit 1
}
choose() {
    # wtype's temporary keymap does not reliably activate GTK default buttons.
    # Use the native accessibility actions, in Sway's D-Bus session, instead.
    rm -f "$scratch/chooser-done"
    swaymsg "exec /usr/bin/python3 '$chooser' '$1' '$scratch/chooser-done' > '$scratch/chooser.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        [[ -f "$scratch/chooser-done" ]] && break
        sleep 0.1
    done
    if [[ ! -f "$scratch/chooser-done" ]]; then
        cat "$scratch/chooser.log" >&2
        exit 1
    fi
    wait_for_dialog_close
}
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep 0.5
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}

expect_title Review
capture open-empty
open_dialog
capture open-native-dialog
choose cancel
expect_title Review
open_dialog
choose "$scratch/résumé 日本語 document.pdf"
expect_title 'Review — résumé 日本語 document.pdf — 1/2 — 100%'
capture open-loaded
key Right
key equal
expect_title 'Review — résumé 日本語 document.pdf — 2/2 — 125%'
open_dialog
choose cancel
expect_title 'Review — résumé 日本語 document.pdf — 2/2 — 125%'
open_dialog
choose "$scratch/broken.pdf"
expect_title 'Review — résumé 日本語 document.pdf — 2/2 — 125%'
capture open-error
key Escape
expect_title 'Review — résumé 日本語 document.pdf — 2/2 — 125%'
open_dialog
choose "$scratch/outline.pdf"
expect_title 'Review — outline.pdf — 1/2 — 100%'
capture open-replaced
echo 'PASS: native open, Unicode path, cancel, failure recovery, and document replacement'
