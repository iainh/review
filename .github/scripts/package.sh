#!/usr/bin/env bash
set -euo pipefail

target="${1:?usage: package.sh <target-triple>}"
version=$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[0].version')
if [[ "${GITHUB_REF:-}" == refs/tags/* && "${GITHUB_REF_NAME:-}" != "v${version}" ]]; then
  echo "Release tag must match Cargo.toml version: v${version}" >&2
  exit 1
fi

name="review-${version}-${target}"
executable=review
case "$target" in
  x86_64-unknown-linux-gnu) extension=tar.gz ;;
  aarch64-apple-darwin|x86_64-apple-darwin) extension=dmg ;;
  x86_64-pc-windows-msvc) extension=zip; executable=review.exe ;;
  *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/$name" dist
cp "target/$target/release/$executable" README.md LICENSE "$staging/$name/"
archive="$PWD/dist/$name.$extension"
rm -f "$archive"

# Help must run from the packaged executable without a display or missing libraries.
smoke_test() {
  local output
  output=$("$1" --help 2>&1)
  printf '%s\n' "$output"
  grep -qi 'usage:' <<< "$output"
}

case "$extension" in
  tar.gz)
    cp platform/linux/review.desktop platform/linux/install-desktop.sh \
      platform/linux/uninstall-desktop.sh "$staging/$name/"
    chmod +x "$staging/$name/"*.sh
    tar -czf "$archive" -C "$staging" "$name"
    mkdir "$staging/extracted"
    tar -xzf "$archive" -C "$staging/extracted"
    smoke_test "$staging/extracted/$name/$executable"
    ;;
  dmg)
    bundle="$staging/$name/Review.app"
    mkdir -p "$bundle/Contents/MacOS"
    mv "$staging/$name/$executable" "$bundle/Contents/MacOS/"
    cp platform/macos/Info.plist "$bundle/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$bundle/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$bundle/Contents/Info.plist"
    plutil -lint "$bundle/Contents/Info.plist"
    codesign --force --sign - "$bundle"
    codesign --verify --strict "$bundle"
    ln -s /Applications "$staging/$name/Applications"
    hdiutil create -volname "Review $version" -srcfolder "$staging/$name" \
      -format UDZO -ov "$archive"
    hdiutil verify "$archive"
    mkdir "$staging/mounted"
    hdiutil attach "$archive" -readonly -nobrowse -mountpoint "$staging/mounted"
    trap 'hdiutil detach "$staging/mounted"; rm -rf "$staging"' EXIT
    codesign --verify --strict "$staging/mounted/Review.app"
    smoke_test "$staging/mounted/Review.app/Contents/MacOS/$executable"
    hdiutil detach "$staging/mounted"
    trap 'rm -rf "$staging"' EXIT
    ;;
  zip)
    cp platform/windows/Install-FileAssociation.ps1 \
      platform/windows/Uninstall-FileAssociation.ps1 "$staging/$name/"
    (cd "$staging" && 7z a -tzip "$archive" "$name")
    7z t "$archive"
    7z x "$archive" "-o$staging/extracted"
    smoke_test "$staging/extracted/$name/$executable"
    ;;
esac

echo "Verified package: $archive"
