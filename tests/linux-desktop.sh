#!/usr/bin/env bash
set -euo pipefail

# Exercise the desktop entry parser/launcher, not just its text. All writes
# stay in a disposable XDG home; the real user's associations are untouched.
root=$(dirname -- "$(dirname -- "$(realpath -- "${BASH_SOURCE[0]}")")")
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
export HOME="$scratch/home" XDG_DATA_HOME="$scratch/data" XDG_CONFIG_HOME="$scratch/config"
export REVIEW_TEST_OUTPUT="$scratch/arguments"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME/applications"
printf '[Default Applications]\napplication/pdf=other.desktop;\n' > "$XDG_CONFIG_HOME/mimeapps.list"
cp "$XDG_CONFIG_HOME/mimeapps.list" "$scratch/original-defaults"
printf '[Desktop Entry]\nType=Application\nName=Other\nExec=true\n' > "$XDG_DATA_HOME/applications/other.desktop"

package="$scratch/Review résumé \"quoted\" \$dollar \`tick\` %f \\ slash"
mkdir -p -- "$package"
cp "$root"/platform/linux/* "$package/"
cp "$root/assets/review-256.png" "$package/review.png"
cat > "$package/review" <<'SCRIPT'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$REVIEW_TEST_OUTPUT"
SCRIPT
chmod +x "$package/review"
pdf="$scratch/résumé 日本語 file.pdf"
touch "$pdf"

for _ in 1 2; do
    bash "$package/install-desktop.sh"
done
entry="$XDG_DATA_HOME/applications/review.desktop"
icon="$XDG_DATA_HOME/icons/hicolor/256x256/apps/review.png"
cmp "$root/assets/review-256.png" "$icon"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$entry"
fi
gio launch "$entry" "$pdf"
for _ in {1..50}; do
    [[ -f "$REVIEW_TEST_OUTPUT" ]] && break
    sleep 0.1
done
printf '%s\n' -- "$pdf" > "$scratch/expected"
cmp "$scratch/expected" "$REVIEW_TEST_OUTPUT"
cmp "$scratch/original-defaults" "$XDG_CONFIG_HOME/mimeapps.list"
echo 'PASS: desktop launch preserves special executable characters and Unicode PDF paths'

for _ in 1 2; do
    bash "$package/uninstall-desktop.sh"
done
[[ ! -e "$entry" && ! -e "$icon" && -e "$XDG_DATA_HOME/applications/other.desktop" ]]
cmp "$scratch/original-defaults" "$XDG_CONFIG_HOME/mimeapps.list"
echo 'PASS: registration/removal are idempotent and preserve existing defaults and other apps'
