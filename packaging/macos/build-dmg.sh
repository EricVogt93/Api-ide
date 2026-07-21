#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: build-dmg.sh VERSION}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="$root/target/release-packages"
app="$out/Forge.app"
iconset="$out/Forge.iconset"
stage="$out/dmg-root"
arch="$(uname -m)"
dmg="$out/Forge-$version-macOS-$arch.dmg"

cd "$root"
cargo build --release --locked -p forge-gui --bin forge-ide

rm -rf "$app" "$iconset" "$stage" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$iconset" "$stage"
install -m755 target/release/forge-ide "$app/Contents/MacOS/forge-ide"
install -m644 packaging/macos/Info.plist "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$app/Contents/Info.plist"

for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" \
  "64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" \
  "256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" \
  "1024 icon_512x512@2x"; do
  read -r size name <<< "$spec"
  sips -z "$size" "$size" crates/forge-gui/assets/logo-dark.png \
    --out "$iconset/$name.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Forge.icns"
codesign --force --deep --sign - "$app"

cp -R "$app" "$stage/Forge.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "Forge" -srcfolder "$stage" -ov -format UDZO "$dmg"
test -s "$dmg"
