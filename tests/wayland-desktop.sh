#!/usr/bin/env bash
set -euo pipefail

# Disposable 1280×900 native Sway session (tests/sway.conf), shared D-Bus.
: "${WAYLAND_DISPLAY:?Run inside the native Wayland test session}"
: "${SWAYSOCK:?Set the disposable Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; swaymsg "[app_id=\"^zenity$\"] kill" >/dev/null; rm -rf "$scratch"' EXIT
cargo build --locked
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_wayland_fixture -- --ignored
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked export_forms_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
checker=$(realpath tests/desktop-accessibility.py)
captures=${REVIEW_SCREENSHOTS:-$scratch/captures}
swaymsg "exec bash -c '/usr/bin/python3 \"$checker\" \"$binary\" \"$scratch\" \"$captures\" \"$scratch/pointer\" > \"$scratch/check.log\" 2>&1; echo \$? > \"$scratch/status\"'" >/dev/null
for _ in {1..1800}; do
    if [[ -f "$scratch/status" ]]; then break; fi
    sleep .1
done
cat "$scratch/check.log"
[[ -f "$scratch/status" && $(cat "$scratch/status") == 0 ]]
REVIEW_FIXTURE_DIR="$scratch" cargo test --locked verify_native_desktop_save -- --ignored
echo 'PASS: reopened desktop-menu save contains the entered form value'
