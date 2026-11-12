#!/usr/bin/env bash
# Build an unsigned (ad-hoc signed) Wholeseek.app and DMG for Apple Silicon Macs.
# Writes to target/macos-dist/ (gitignored). Needs Xcode command line tools + rustup.
#   packaging/macos/build-dmg.sh               # build, bundle, DMG
#   SKIP_BUILD=1 packaging/macos/build-dmg.sh  # reuse target/aarch64-apple-darwin/release
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cd "$root"

# Apple Silicon only.
targets=(aarch64-apple-darwin)
flavour=apple-silicon
rustup target add "${targets[@]}" >/dev/null
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0}
[ "$(uname -m)" = arm64 ] || echo "note: building an Apple Silicon app on an Intel Mac (it will not run here)" >&2

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

"$here/make-icns.sh" assets/wholeseek.svg "$app/Contents/Resources/wholeseek.icns" assets/wholeseek-1024.png

# Ad-hoc signature: required for arm64 binaries to run; not notarized, so
# Gatekeeper asks once (right-click → Open).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"

dmg="$dist/wholeseek-$version-macos-$flavour.dmg"
stage=$(mktemp -d)
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
# hdiutil sometimes fails with "Resource busy"; retry.
for try in 1 2 3 4 5; do
  if hdiutil create -volname "Wholeseek $version" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"; then
    break
  fi
  [ "$try" = 5 ] && { echo "hdiutil create failed 5 times" >&2; exit 1; }
  echo "hdiutil create failed (try $try), retrying" >&2
  sleep $((try * 5))
done
rm -rf "$stage"
hdiutil verify "$dmg"

echo "dmg: $dmg ($(wc -c <"$dmg" | tr -d ' ') bytes)"
