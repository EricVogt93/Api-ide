#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: build-appimage.sh VERSION}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
appdir="$root/target/appimage/Forge.AppDir"
out="$root/target/release-packages"
linuxdeploy="${LINUXDEPLOY:-$root/target/tools/linuxdeploy-x86_64.AppImage}"

cd "$root"
cargo build --release --locked -p forge-gui --bin forge-ide

if [[ ! -x "$linuxdeploy" ]]; then
  mkdir -p "$(dirname "$linuxdeploy")"
  curl --fail --location --output "$linuxdeploy" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
  chmod +x "$linuxdeploy"
fi

rm -rf "$appdir"
mkdir -p "$out"
install -Dm755 target/release/forge-ide "$appdir/usr/bin/forge-ide"
install -Dm644 README.md "$appdir/usr/share/doc/forge/README.md"
install -Dm644 packaging/linux/com.ericvogt.forge.appdata.xml \
  "$appdir/usr/share/metainfo/com.ericvogt.forge.appdata.xml"

export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="$out/Forge-$version-linux-x86_64.AppImage"
"$linuxdeploy" \
  --appdir "$appdir" \
  --desktop-file packaging/linux/com.ericvogt.forge.desktop \
  --icon-file packaging/linux/forge-ide-symbolic.svg \
  --output appimage
test -s "$LDAI_OUTPUT"
