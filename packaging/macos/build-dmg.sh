#!/usr/bin/env bash
# Build an unsigned (ad-hoc signed) universal Wholeseek.app and DMG.
# Run on macOS from anywhere; writes to target/macos-dist/ (gitignored).
#   packaging/macos/build-dmg.sh            # build both arches, bundle, DMG
#   SKIP_BUILD=1 packaging/macos/build-dmg.sh   # reuse target/<triple>/release
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cd "$root"

targets=(aarch64-apple-darwin x86_64-apple-darwin)
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0}

version=$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="wholeseek"))')
echo "wholeseek $version"

if [ -z "${SKIP_BUILD:-}" ]; then
  for t in "${targets[@]}"; do
    cargo build --release --locked -p wholeseek --target "$t"
  done
fi

dist="$root/target/macos-dist"
app="$dist/Wholeseek.app"
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

lipo -create -output "$app/Contents/MacOS/wholeseek" \
  $(for t in "${targets[@]}"; do echo "target/$t/release/wholeseek"; done)
lipo -info "$app/Contents/MacOS/wholeseek"

sed "s/@VERSION@/$version/g" "$here/Info.plist" >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' >"$app/Contents/PkgInfo"

"$here/make-icns.sh" assets/wholeseek.svg "$app/Contents/Resources/wholeseek.icns" assets/wholeseek-256.png

# Ad-hoc signature: required for arm64 binaries to run; not notarized, so
# Gatekeeper asks once (right-click → Open).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

dmg="$dist/wholeseek-$version-macos-universal.dmg"
stage=$(mktemp -d)
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "Wholeseek $version" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
rm -rf "$stage"
hdiutil verify "$dmg"

echo "dmg: $dmg ($(wc -c <"$dmg" | tr -d ' ') bytes)"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "dmg=$dmg"; echo "name=$(basename "$dmg")"; echo "version=$version"; } >>"$GITHUB_OUTPUT"
fi
