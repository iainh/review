#!/usr/bin/env bash
set -euo pipefail

# Disposable Sway session, 1280×900 scale 1, with a session D-Bus and GTK chooser.
: "${WAYLAND_DISPLAY:?Run in a Wayland Sway session}"
: "${SWAYSOCK:?Set the test Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_inspection_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test export_wayland_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
chooser=$(realpath tests/gtk-chooser.py)
original=$(sha256sum "$scratch/inspection.pdf" | cut -d' ' -f1)

launch() {
    rm -f "$scratch/viewer-exit"
    swaymsg "exec bash -c 'env -u DISPLAY WINIT_UNIX_BACKEND=wayland GDK_BACKEND=wayland \"$binary\" \"$scratch/$1.pdf\"; printf \"%s\\n\" \$? > \"$scratch/viewer-exit\"'" >/dev/null
}
close_viewer() {
    swaymsg '[app_id="^review$"] kill' >/dev/null
    for _ in {1..100}; do
        if [[ -f "$scratch/viewer-exit" ]]; then
            [[ $(cat "$scratch/viewer-exit") == 0 ]]
            echo 'PASS: native viewer shutdown exits cleanly'
            return
        fi
        sleep .1
    done
    echo 'Viewer did not exit' >&2
    exit 1
}
title() { swaymsg -t get_tree | jq -r '.. | objects | select(.app_id? == "review") | .name'; }
expect_page() {
    for _ in {1..100}; do
        if [[ $(title) == *" — $1/2 [$2] — "* ]]; then
            echo "PASS: physical page $1, label $2"
            return
        fi
        sleep .1
    done
    echo "Wrong page: $(title)" >&2
    exit 1
}
key() { wtype -s 150 -k "$1" -s 150; }
inspect() { wtype -s 150 -M ctrl -k d -m ctrl -s 150; }
go() { wtype -s 150 -M ctrl -k g -m ctrl -s 150 "$1" -s 150 -k Return -s 150; }
capture() {
    sleep .5
    grim "$scratch/$1.png"
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        cp "$scratch/$1.png" "$REVIEW_SCREENSHOTS/$1.png"
    fi
}
choose() {
    rm -f "$scratch/chooser-done"
    swaymsg "exec /usr/bin/python3 '$chooser' '$1' '$scratch/chooser-done' > '$scratch/chooser.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        if [[ -f "$scratch/chooser-done" ]]; then sleep .5; return; fi
        sleep .1
    done
    cat "$scratch/chooser.log" >&2
    exit 1
}
save_dialog() {
    "$scratch/pointer" click 53 268
    for _ in {1..100}; do
        if swaymsg -t get_tree | jq -e '.. | objects | select(.app_id? == "zenity" or .name? == "Save attachment (will not open)")' >/dev/null; then return; fi
        sleep .1
    done
    echo 'Save chooser did not appear' >&2
    exit 1
}
red_pixels() {
    magick "$1" -crop "${2:-1280x900+0+0}" -format %c histogram:info: | awk '/#FF0000 / {sum += $1} END {print sum+0}'
}

launch inspection
expect_page 1 iv
go A-1
expect_page 2 A-1
go iv
expect_page 1 iv
go 2
expect_page 2 A-1
go missing-label
expect_page 2 A-1
go 1
expect_page 1 iv
inspect
capture inspection-properties
"$scratch/pointer" click 279 65
capture inspection-signatures
"$scratch/pointer" click 142 65
capture inspection-attachments
save_dialog
capture inspection-save-dialog
choose cancel
[[ ! -f "$scratch/payload.txt" ]]
save_dialog
choose "$scratch/chosen.txt"
printf 'embedded payload\n' > "$scratch/expected.txt"
cmp "$scratch/chosen.txt" "$scratch/expected.txt"
capture inspection-saved
echo 'PASS: explicit save produces exact bytes; cancel creates no file'

"$scratch/pointer" click 211 65
# Expose the sidebar while leaving the main page's red rectangle visible below
# the inspector. Pixel checks now catch stale thumbnails as well as stale pages.
"$scratch/pointer" drag 340 33 910 33
"$scratch/pointer" click 79 33
capture inspection-layers-on
[[ $(red_pixels "$scratch/inspection-layers-on.png") -gt 0 ]]
[[ $(red_pixels "$scratch/inspection-layers-on.png" 170x100+640+590) -gt 0 ]]
[[ $(red_pixels "$scratch/inspection-layers-on.png" 240x250+0+40) -gt 0 ]]
"$scratch/pointer" click 599 163
capture inspection-layers-off
[[ $(red_pixels "$scratch/inspection-layers-off.png") -eq 0 ]]
"$scratch/pointer" click 599 163
capture inspection-layers-restored
[[ $(red_pixels "$scratch/inspection-layers-restored.png") -gt 0 ]]
[[ $(sha256sum "$scratch/inspection.pdf" | cut -d' ' -f1) == "$original" ]]
key Escape
expect_page 1 iv
echo 'PASS: native layer toggle removes/restores rendered pixels; source PDF unchanged; Escape closes inspector without quitting'

# The outline fixture has none of these inspection features.
close_viewer
launch outline
for _ in {1..100}; do [[ $(title) == *'outline.pdf'* ]] && break; sleep .1; done
inspect
"$scratch/pointer" click 142 65
capture inspection-empty-attachments
"$scratch/pointer" click 211 65
capture inspection-empty-layers
"$scratch/pointer" click 279 65
capture inspection-empty-signatures
echo 'PASS: rendered empty inspector tabs'
close_viewer
