#!/usr/bin/env bash
set -euo pipefail

# Install the latest Forge AppImage in Desktop Mode and expose it to Steam.
# Usage: curl -fsSL https://raw.githubusercontent.com/EricVogt93/Api-ide/main/scripts/install-steamdeck.sh | bash

repo="${FORGE_REPO:-EricVogt93/Api-ide}"
base="${XDG_DATA_HOME:-$HOME/.local/share}/forge"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
api="https://api.github.com/repos/$repo/releases/latest"

command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required (install it with pacman -S jq)" >&2; exit 1; }

asset_url="$(curl -fsSL "$api" | jq -r '.assets[] | select(.name | endswith("-linux-x86_64.AppImage")) | .browser_download_url' | head -n1)"
[[ -n "$asset_url" && "$asset_url" != "null" ]] || { echo "No Linux AppImage found in the latest release." >&2; exit 1; }

mkdir -p "$base" "$bin_dir" "$desktop_dir"
app="$base/Forge.AppImage"
tmp="$app.part"
curl -fL --retry 3 --progress-bar "$asset_url" -o "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$app"
ln -sfn "$app" "$bin_dir/forge"

desktop="$desktop_dir/com.ericvogt.forge.desktop"
cat > "$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Forge
GenericName=API IDE
Comment=Build, inspect, and verify APIs
Exec=$app
Icon=application-x-executable
Terminal=false
Categories=Development;
StartupNotify=true
EOF
chmod 644 "$desktop"

if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$desktop_dir" >/dev/null 2>&1 || true
fi

# SteamOS provides this helper to create a Non-Steam shortcut. It is absent on
# ordinary Linux desktops, where the .desktop launcher remains sufficient.
if command -v steamos-add-to-steam >/dev/null; then
  steamos-add-to-steam "$desktop" || echo "Steam shortcut registration failed; launch the desktop entry once and add it manually in Steam."
else
  echo "steamos-add-to-steam not found; the desktop launcher was installed."
  echo "In Steam: Games → Add a Non-Steam Game → Browse → $app"
fi

echo "Forge installed: $app"
echo "Launch with: $bin_dir/forge"
