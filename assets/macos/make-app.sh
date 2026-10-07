#!/usr/bin/env bash
# Assembles FileCargo.app around a built GUI binary: Info.plist, icon, ad-hoc signature.
# Usage: make-app.sh <filecargo binary> <version> <out dir>
set -euo pipefail

if [ $# -ne 3 ]; then
  echo "usage: make-app.sh <filecargo binary> <version> <out dir>" >&2
  exit 2
fi
binary="$1"
version="$2"
out="$3"
[ -f "$binary" ] || { echo "make-app.sh: no such binary: $binary" >&2; exit 1; }
[ -n "$version" ] || { echo "make-app.sh: empty version" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
icon="$here/../icons/filecargo.icns"
[ -f "$icon" ] || { echo "make-app.sh: missing $icon (run assets/icons.sh)" >&2; exit 1; }

# the deployment target the binary was really built for (Intel and arm64 builds differ)
min_macos="$(vtool -show-build "$binary" | awk '$1 == "minos" { print $2; exit }')"
[ -n "$min_macos" ] || { echo "make-app.sh: cannot read minos from $binary" >&2; exit 1; }

app="$out/FileCargo.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/filecargo"
cp "$icon" "$app/Contents/Resources/filecargo.icns"
sed -e "s/__VERSION__/$version/g" -e "s/__MIN_MACOS__/$min_macos/g" \
  "$here/Info.plist" > "$app/Contents/Info.plist"

plutil -lint "$app/Contents/Info.plist"
# sign the finished bundle, never its parts
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "make-app.sh: $app (macOS $min_macos+)"
