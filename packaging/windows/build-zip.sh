#!/usr/bin/env bash
# Build target/windows-dist/wholeseek-<ver>-windows-x86_64.zip on Windows (Git Bash, as on the
# windows-latest runner). Needs rustup + the MSVC build tools (Windows SDK rc.exe embeds the
# icon and version info; the build fails without it) and 7-Zip or zip.
#   packaging/windows/build-zip.sh               # cargo build --release, then pack
#   SKIP_BUILD=1 packaging/windows/build-zip.sh  # pack target/release/wholeseek.exe as is
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# `pwd -W` (Git Bash): D:/a/... rather than /d/a/..., so the path in GITHUB_OUTPUT also works
# for actions that are not run by bash (upload-artifact).
root=$(cd "$here/../.." && { pwd -W 2>/dev/null || pwd; })
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
echo "wholeseek $version (windows x86_64)"

if [ -z "${SKIP_BUILD:-}" ]; then
  # Static C runtime: the exe must not need VCRUNTIME140.dll (the Visual C++ Redistributable is
  # not on every Windows); the Universal CRT ships with Windows 10+.
  RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" \
    WHOLESEEK_REQUIRE_RESOURCES=1 cargo build --release --locked -p wholeseek
fi
exe=target/release/wholeseek.exe
[ -f "$exe" ] || { echo "no $exe" >&2; exit 1; }

dist="$root/target/windows-dist"
name="wholeseek-$version-windows-x86_64"
pkg="$dist/stage/wholeseek"
rm -rf "$dist"
mkdir -p "$pkg"
cp "$exe" "$pkg/wholeseek.exe"
# CRLF so Notepad shows the lines on any Windows.
sed 's/\r*$/\r/' "$here/README.txt" >"$pkg/README.txt"
sed 's/\r*$/\r/' LICENSE >"$pkg/LICENSE.txt"

zip="$dist/$name.zip"
if command -v 7z >/dev/null; then
  (cd "$dist/stage" && 7z a -tzip -mx=9 "$zip" wholeseek >/dev/null)
elif command -v zip >/dev/null; then
  (cd "$dist/stage" && zip -9 -r -q "$zip" wholeseek)
else
  echo "need 7z or zip" >&2
  exit 1
fi
rm -rf "$dist/stage"
echo "zip: $zip ($(wc -c <"$zip" | tr -d ' ') bytes)"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "zip=$zip"; echo "name=$name.zip"; echo "version=$version"; } >>"$GITHUB_OUTPUT"
fi
