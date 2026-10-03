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

# Starting without a PDF must reach the argument parser, not fail to load a library.
smoke_test() {
  local output
  if output=$("$1" 2>&1); then
    echo "Expected the executable to reject a missing PDF argument" >&2
    return 1
  fi
  printf '%s\n' "$output"
  grep -q 'usage: .* <document.pdf>' <<< "$output"
}

case "$extension" in
  tar.gz)
    tar -czf "$archive" -C "$staging" "$name"
    mkdir "$staging/extracted"
    tar -xzf "$archive" -C "$staging/extracted"
    smoke_test "$staging/extracted/$name/$executable"
    ;;
  dmg)
    hdiutil create -volname "Review $version" -srcfolder "$staging/$name" \
      -format UDZO -ov "$archive"
    hdiutil verify "$archive"
    mkdir "$staging/mounted"
    hdiutil attach "$archive" -readonly -nobrowse -mountpoint "$staging/mounted"
    trap 'hdiutil detach "$staging/mounted"; rm -rf "$staging"' EXIT
    smoke_test "$staging/mounted/$executable"
    hdiutil detach "$staging/mounted"
    trap 'rm -rf "$staging"' EXIT
    ;;
  zip)
    (cd "$staging" && 7z a -tzip "$archive" "$name")
    7z t "$archive"
    7z x "$archive" "-o$staging/extracted"
    smoke_test "$staging/extracted/$name/$executable"
    ;;
esac

echo "Verified package: $archive"
