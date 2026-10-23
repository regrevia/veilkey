#!/usr/bin/env bash
set -euo pipefail

target="${1:?target triple required}"
version="${2:?version required}"
out_dir="${3:?output directory required}"
root_dir="$(cd "$(dirname "$0")/../.." && pwd)"
app_dir="$out_dir/VeilKey.app"

rm -rf "$app_dir"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$root_dir/target/$target/release/veilkey" "$app_dir/Contents/MacOS/VeilKey"
cp "$root_dir/assets/icons/VeilKey.icns" "$app_dir/Contents/Resources/VeilKey.icns"
chmod 755 "$app_dir/Contents/MacOS/VeilKey"
cat > "$app_dir/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>VeilKey</string>
  <key>CFBundleDisplayName</key><string>VeilKey</string>
  <key>CFBundleIdentifier</key><string>cool.cnb.regrevia.veilkey</string>
  <key>CFBundleExecutable</key><string>VeilKey</string>
  <key>CFBundleIconFile</key><string>VeilKey.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
ditto -c -k --sequesterRsrc --keepParent "$app_dir" "$out_dir/VeilKey-$version-$target.zip"
