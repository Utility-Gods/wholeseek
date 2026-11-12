#!/usr/bin/env bash
# Build wholeseek.icns from assets/wholeseek.svg (macOS only: needs iconutil).
# Usage: make-icns.sh <svg> <out.icns> [fallback.png]
# Renders with rsvg-convert when available (brew install librsvg); otherwise
# scales the 1024 px fallback PNG down with sips (built into macOS).
set -euo pipefail
svg=$1 out=$2 png=${3:-}
work=$(mktemp -d)
set_dir="$work/wholeseek.iconset"
mkdir -p "$set_dir"
render() { # <px> <file>
  if command -v rsvg-convert >/dev/null; then
    rsvg-convert -w "$1" -h "$1" -o "$2" "$svg"
  elif [ -n "$png" ]; then
    sips -z "$1" "$1" "$png" --out "$2" >/dev/null
  else
    echo "need rsvg-convert or a fallback PNG" >&2; exit 1
  fi
}
for s in 16 32 128 256 512; do
  render "$s" "$set_dir/icon_${s}x${s}.png"
  render "$((s * 2))" "$set_dir/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$set_dir" -o "$out"
rm -rf "$work"
echo "icon: $out ($(wc -c <"$out" | tr -d ' ') bytes)"
