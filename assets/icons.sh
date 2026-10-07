#!/usr/bin/env bash
# Regenerates every derived icon under assets/icons/ from assets/logo.svg (macOS: needs iconutil).
# Outputs are committed; run this after any change to logo.svg.
set -euo pipefail

for tool in rsvg-convert magick iconutil; do
  command -v "$tool" > /dev/null || { echo "icons.sh: missing tool: $tool" >&2; exit 1; }
done

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
svg="$here/logo.svg"
out="$here/icons"
png="$out/png"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

rm -rf "$out"
mkdir -p "$png"

render() { rsvg-convert -w "$1" -h "$1" "$svg" -o "$2"; }

# Full-bleed PNGs (Linux hicolor, X11 window icon) and the Windows .ico source sizes.
for n in 16 24 32 48 64 128 256 512; do
  render "$n" "$png/filecargo-$n.png"
done
magick "$png/filecargo-16.png" "$png/filecargo-24.png" "$png/filecargo-32.png" \
  "$png/filecargo-48.png" "$png/filecargo-64.png" "$png/filecargo-128.png" \
  "$png/filecargo-256.png" "$out/filecargo.ico"

# macOS: Apple grid, the tile at 824/1024 of the canvas, centered on a transparent margin.
set="$work/filecargo.iconset"
mkdir "$set"
apple() { # <canvas px> <file>
  local tile=$(( $1 * 824 / 1024 ))
  render "$tile" "$work/tile.png"
  magick "$work/tile.png" -background none -gravity center -extent "${1}x${1}" "$2"
}
for n in 16 32 128 256 512; do
  apple "$n" "$set/icon_${n}x${n}.png"
  apple $(( n * 2 )) "$set/icon_${n}x${n}@2x.png"
done
iconutil -c icns "$set" -o "$out/filecargo.icns"

# Size check.
for n in 16 24 32 48 64 128 256 512; do
  [ "$(magick identify -format '%wx%h' "$png/filecargo-$n.png")" = "${n}x${n}" ] \
    || { echo "icons.sh: filecargo-$n.png has the wrong size" >&2; exit 1; }
done
ico_sizes="$(magick identify -format '%w ' "$out/filecargo.ico")"
[ "$ico_sizes" = "16 24 32 48 64 128 256 " ] \
  || { echo "icons.sh: filecargo.ico has sizes: $ico_sizes" >&2; exit 1; }
iconutil -c iconset "$out/filecargo.icns" -o "$work/check.iconset"
[ "$(ls "$work/check.iconset" | wc -l | tr -d ' ')" = 10 ] \
  || { echo "icons.sh: filecargo.icns does not hold 10 images" >&2; exit 1; }
echo "icons.sh: ok"
