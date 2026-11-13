#!/usr/bin/env bash
# Wrap a release binary in a macOS .app bundle so it shows up in Spotlight/Launchpad
# and can be associated with .parquet files.
#
# Usage: scripts/bundle-macos.sh <path/to/parqueter-binary> <version> <output-dir>
# Produces: <output-dir>/Parqueter.app
set -euo pipefail

binary="$1"
version="$2"
out_dir="$3"

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$out_dir/Parqueter.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$binary" "$app/Contents/MacOS/parqueter"
chmod +x "$app/Contents/MacOS/parqueter"
sed "s/__VERSION__/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

if [ -f "$root/packaging/macos/AppIcon.icns" ]; then
  cp "$root/packaging/macos/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
  plutil -insert CFBundleIconFile -string AppIcon "$app/Contents/Info.plist"
fi

# Ad-hoc sign the whole bundle so Info.plist is sealed into the signature.
# This is not a Developer ID signature: it does not satisfy Gatekeeper for quarantined
# downloads, but it is all an app installed via curl (no quarantine flag) needs.
codesign --force --sign - "$app"
codesign --verify --strict "$app"

echo "Built $app"
