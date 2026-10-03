#!/usr/bin/env bash
set -euo pipefail

# In release packages this script and review.desktop sit beside review.
directory=$(dirname -- "$(realpath -- "${BASH_SOURCE[0]}")")
binary=$(realpath -- "${1:-$directory/review}")
if [[ ! -x "$binary" ]]; then
    echo "Expected a Review executable at $binary" >&2
    echo "Usage: bash install-desktop.sh [/absolute/path/to/review]" >&2
    exit 1
fi
if [[ "$binary" == *$'\n'* || "$binary" == *$'\r'* ]]; then
    echo "The executable path cannot contain line breaks" >&2
    exit 1
fi

# Escape the quoted Exec argument, then the desktop entry's string layer.
escaped=${binary//\\/\\\\}
escaped=${escaped//\"/\\\"}
escaped=${escaped//\$/\\\$}
escaped=${escaped//\`/\\\`}
escaped=${escaped//\\/\\\\}
escaped=${escaped//%/%%}
applications=${XDG_DATA_HOME:-$HOME/.local/share}/applications
mkdir -p -- "$applications"
while IFS= read -r line; do
    if [[ "$line" == Exec=* ]]; then
        # GLib checks the executable before expanding %% in field codes. Use
        # env so paths containing percent signs are arguments, not that check.
        printf 'Exec=env "%s" -- %%f\n' "$escaped"
    else
        printf '%s\n' "$line"
    fi
done < "$directory/review.desktop" > "$applications/review.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$applications"
fi
echo "Registered Review for PDF Open With menus. Your default viewer is unchanged."
echo "Keep the executable at: $binary"
