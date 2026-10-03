#!/usr/bin/env bash
set -euo pipefail

applications=${XDG_DATA_HOME:-$HOME/.local/share}/applications
rm -f -- "$applications/review.desktop"
if [[ -d "$applications" ]] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications"
fi
echo "Removed Review's desktop registration. If it was your default, choose another PDF viewer."
