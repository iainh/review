#!/usr/bin/env bash
set -euo pipefail

# Run on the shared D-Bus of a disposable native Sway session, 1280×900 scale 1.
: "${WAYLAND_DISPLAY:?Run inside the native Wayland test session}"
: "${SWAYSOCK:?Set the disposable Sway IPC socket}"
scratch=$(mktemp -d)
trap 'set +e; swaymsg "[app_id=\"^review$\"] kill" >/dev/null; wl-copy --clear; rm -rf "$scratch"' EXIT
cargo build
REVIEW_FIXTURE_DIR="$scratch" cargo test export_forms_fixture -- --ignored
curl -fsSL 'https://raw.githubusercontent.com/swaywm/wlr-protocols/b010a03648b88d143236de193bddbfea0c08bc84/unstable/wlr-virtual-pointer-unstable-v1.xml' -o "$scratch/virtual-pointer.xml"
wayland-scanner client-header "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.h"
wayland-scanner private-code "$scratch/virtual-pointer.xml" "$scratch/virtual-pointer.c"
cc -Wall -Wextra -Werror -I"$scratch" tests/wayland-pointer.c "$scratch/virtual-pointer.c" \
    $(pkg-config --cflags --libs wayland-client) -o "$scratch/pointer"
binary=$(realpath target/debug/review)
checker=$(realpath tests/forms-accessibility.py)
captures=${REVIEW_SCREENSHOTS:-$scratch/captures}
swaymsg "exec bash -c '/usr/bin/python3 \"$checker\" \"$binary\" \"$scratch\" \"$captures\" \"$scratch/pointer\" > \"$scratch/check.log\" 2>&1; echo \$? > \"$scratch/status\"'" >/dev/null
for _ in {1..1200}; do
    if [[ -f "$scratch/status" ]]; then break; fi
    sleep .1
done
cat "$scratch/check.log"
[[ -f "$scratch/status" && $(cat "$scratch/status") == 0 ]]
REVIEW_FIXTURE_DIR="$scratch" cargo test verify_native_forms_output -- --ignored
echo 'PASS: native saved form values reopened, including encrypted fill-only access'
