#!/usr/bin/env bash
set -euo pipefail

applications=${XDG_DATA_HOME:-$HOME/.local/share}/applications
icon=${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps/review.png
rm -f -- "$applications/review.desktop"
rm -f -- "$icon"
if [[ -d "$applications" ]] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications"
fi
echo "Removed Review's desktop registration. If it was your default, choose another PDF viewer."
