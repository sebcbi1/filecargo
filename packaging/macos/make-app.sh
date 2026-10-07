#!/usr/bin/env bash
# Assembles FileCargo.app around a built GUI binary: Info.plist, icon, ad-hoc signature.
# Usage: make-app.sh <filecargo binary> <version> <out dir> [filecargo-tui binary]
# The TUI, when given, goes next to the GUI in Contents/MacOS (users symlink it onto their PATH).
set -euo pipefail

if [ $# -lt 3 ] || [ $# -gt 4 ]; then
  echo "usage: make-app.sh <filecargo binary> <version> <out dir> [filecargo-tui binary]" >&2
  exit 2
fi
binary="$1"
version="$2"
out="$3"
tui="${4:-}"
[ -f "$binary" ] || { echo "make-app.sh: no such binary: $binary" >&2; exit 1; }
[ -z "$tui" ] || [ -f "$tui" ] || { echo "make-app.sh: no such binary: $tui" >&2; exit 1; }
[ -n "$version" ] || { echo "make-app.sh: empty version" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
icon="$here/../../assets/icons/filecargo.icns"
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
# a second executable is nested code: it is signed on its own first, then the finished bundle
if [ -n "$tui" ]; then
  cp "$tui" "$app/Contents/MacOS/filecargo-tui"
  codesign --force --sign - "$app/Contents/MacOS/filecargo-tui"
fi
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "make-app.sh: $app (macOS $min_macos+)"
