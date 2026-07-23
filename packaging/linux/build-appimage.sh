#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: build-appimage.sh VERSION}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
appdir="$root/target/appimage/ApiWright.AppDir"
out="$root/target/release-packages"
linuxdeploy="${LINUXDEPLOY:-$root/target/tools/linuxdeploy-x86_64.AppImage}"

cd "$root"
cargo build --release --locked -p forge-gui --bin apiwright-ide

if [[ ! -x "$linuxdeploy" ]]; then
  mkdir -p "$(dirname "$linuxdeploy")"
  curl --fail --location --output "$linuxdeploy" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
  chmod +x "$linuxdeploy"
fi

rm -rf "$appdir"
mkdir -p "$out"
install -Dm755 target/release/apiwright-ide "$appdir/usr/bin/apiwright-ide"
install -Dm644 README.md "$appdir/usr/share/doc/apiwright/README.md"
install -Dm644 packaging/linux/com.ericvogt.apiwright.appdata.xml \
  "$appdir/usr/share/metainfo/com.ericvogt.apiwright.appdata.xml"

export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="$out/ApiWright-$version-linux-x86_64.AppImage"
"$linuxdeploy" \
  --appdir "$appdir" \
  --desktop-file packaging/linux/com.ericvogt.apiwright.desktop \
  --icon-file packaging/linux/apiwright-symbolic.svg \
  --output appimage
test -s "$LDAI_OUTPUT"
