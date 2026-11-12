#!/usr/bin/env bash
# Build target/linux-dist/wholeseek-<ver>-linux-x86_64.tar.gz.
#   packaging/linux/build-tarball.sh              # cargo build --release, then pack
#   SKIP_BUILD=1 packaging/linux/build-tarball.sh # pack target/release/wholeseek as is
# Icons: hicolor PNGs are rendered from assets/wholeseek.svg when rsvg-convert,
# inkscape or ImageMagick is available; otherwise only 256 px + scalable SVG.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cd "$root"

version=$(cargo metadata --format-version 1 --no-deps |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="wholeseek"))')
arch=$(uname -m)
echo "wholeseek $version ($arch)"

if [ -z "${SKIP_BUILD:-}" ]; then
  cargo build --release --locked -p wholeseek
fi

dist="$root/target/linux-dist"
name="wholeseek-$version-linux-$arch"
pkg="$dist/stage/wholeseek"
rm -rf "$dist"
mkdir -p "$pkg/icons/hicolor/scalable/apps"

install -m 755 target/release/wholeseek "$pkg/wholeseek"
strip "$pkg/wholeseek"
install -m 644 assets/wholeseek.desktop "$pkg/wholeseek.desktop"
install -m 644 assets/wholeseek.svg "$pkg/icons/hicolor/scalable/apps/wholeseek.svg"
install -m 755 "$here/install.sh" "$pkg/install.sh"
install -m 644 LICENSE "$pkg/LICENSE"
install -m 644 "$here/README.txt" "$pkg/README.txt"

render() { # <px> <out.png>
  if command -v rsvg-convert >/dev/null; then
    rsvg-convert -w "$1" -h "$1" -o "$2" assets/wholeseek.svg
  elif command -v inkscape >/dev/null; then
    inkscape -w "$1" -h "$1" -o "$2" assets/wholeseek.svg >/dev/null 2>&1
  elif command -v convert >/dev/null; then
    convert -background none -density 384 assets/wholeseek.svg -resize "${1}x${1}" "$2"
  else
    return 1
  fi
}
for s in 16 24 32 48 64 128 256 512; do
  d="$pkg/icons/hicolor/${s}x${s}/apps"
  mkdir -p "$d"
  if ! render "$s" "$d/wholeseek.png"; then
    rm -rf "$d"
    [ "$s" = 256 ] && { mkdir -p "$d"; install -m 644 assets/wholeseek-256.png "$d/wholeseek.png"; }
  fi
done

find "$pkg/icons" -type f -exec chmod 644 {} +

tarball="$dist/$name.tar.gz"
tar -C "$dist/stage" --owner=0 --group=0 --numeric-owner -czf "$tarball" wholeseek
rm -rf "$dist/stage"
echo "tarball: $tarball ($(wc -c <"$tarball" | tr -d ' ') bytes)"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "tarball=$tarball"; echo "name=$name.tar.gz"; echo "version=$version"; } >>"$GITHUB_OUTPUT"
fi
