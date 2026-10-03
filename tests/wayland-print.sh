#!/usr/bin/env bash
set -euo pipefail

# Disposable native Sway session. Only cancel dialogs and export local PDFs;
# this test never activates Print or submits a job to a printer.
: "${WAYLAND_DISPLAY:?Run in the disposable Sway session described in README.md}"
: "${SWAYSOCK:?Set SWAYSOCK to the test Sway socket}"
scratch=$(mktemp -d)
trap 'swaymsg "[app_id=\"^review$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build --locked
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_wayland_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_print_permission_fixtures -- --ignored
cargo test --locked gtk_exports_print_pages -- --ignored --test-threads=1
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/pointer.xml"
wayland-scanner client-header "$scratch/pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/pointer.xml" "$scratch/pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
inspector=$(realpath tests/gtk-print-dialog.py)
swaymsg 'for_window [title="^Print$"] floating enable' >/dev/null
swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland GDK_BACKEND=wayland '$binary' '$scratch/outline.pdf'" >/dev/null
for _ in {1..100}; do
    swaymsg -t get_tree | jq -e '.. | objects | select(.name? | strings | startswith("Review —"))' >/dev/null && break
    sleep 0.1
done
sleep 0.5
key() { wtype -s 150 -k "$1" -s 150; }
capture() {
    if [[ -n ${REVIEW_SCREENSHOTS:-} ]]; then
        mkdir -p "$REVIEW_SCREENSHOTS"
        sleep 0.6
        grim "$REVIEW_SCREENSHOTS/$1.png"
    fi
}
inspect() {
    rm -f "$scratch/done"
    swaymsg "exec /usr/bin/python3 '$inspector' '$1' '$scratch/done' > '$scratch/inspect.log' 2>&1" >/dev/null
    for _ in {1..100}; do
        [[ -f "$scratch/done" ]] && { cat "$scratch/inspect.log"; return; }
        sleep 0.1
    done
    cat "$scratch/inspect.log" >&2
    exit 1
}
wtype -s 150 -M ctrl -k p -m ctrl -s 150
capture print-fit
key Escape
key Right
swaymsg -t get_tree | jq -e '.. | objects | select(.name? | strings | contains(" — 2/2 — "))' >/dev/null
echo 'PASS: cancelling print options preserves the viewer and navigation'
wtype -s 150 -M ctrl -k p -m ctrl -s 150
"$scratch/pointer" click 613 422
capture print-actual
"$scratch/pointer" click 495 512
for _ in {1..100}; do
    swaymsg -t get_tree | jq -e '.. | objects | select(.name? == "Print")' >/dev/null && break
    sleep 0.1
done
swaymsg '[title="^Print$"] floating enable, resize set 800 px 680 px, move position center' >/dev/null
inspect general
capture print-native-range
inspect setup
capture print-native-setup
inspect cancel
sleep 0.5
! swaymsg -t get_tree | jq -e '.. | objects | select(.name? == "Print")' >/dev/null
swaymsg '[title="^Review —"] focus' >/dev/null
key Left
swaymsg -t get_tree | jq -e '.. | objects | select(.name? | strings | contains(" — 1/2 — "))' >/dev/null
echo 'PASS: native print dialog cancellation returns to the same PDF; no printer job submitted'

for fixture in no-print low-quality; do
    swaymsg '[app_id="^review$"] kill' >/dev/null
    swaymsg "exec env -u DISPLAY WINIT_UNIX_BACKEND=wayland '$binary' '$scratch/$fixture.pdf'" >/dev/null
    sleep 1
    wtype -s 150 -M ctrl -k p -m ctrl -s 150
    capture "print-$fixture"
    # Permission-denied printing leaves navigation usable; low-quality printing
    # opens only our options modal, which Escape dismisses without a native job.
    if [[ $fixture == low-quality ]]; then key Escape; fi
    key Right
    swaymsg -t get_tree | jq -e '.. | objects | select(.name? | strings | contains(" — 2/2 — "))' >/dev/null
done
echo 'PASS: permission-restricted PDFs remain usable; no native print job requested'
