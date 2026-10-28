#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

target=arm64-apple-macosx14.0
if [ "$(uname -m)" = "x86_64" ]; then
    target=x86_64-apple-macosx14.0
fi

# Generate AppIcon.icns from icon-256.png
mkdir -p AppIcon.iconset
sips -z 16 16 assets/icon-256.png --out AppIcon.iconset/icon_16x16.png
sips -z 32 32 assets/icon-256.png --out AppIcon.iconset/icon_16x16@2x.png
sips -z 32 32 assets/icon-256.png --out AppIcon.iconset/icon_32x32.png
sips -z 64 64 assets/icon-256.png --out AppIcon.iconset/icon_32x32@2x.png
sips -z 128 128 assets/icon-256.png --out AppIcon.iconset/icon_128x128.png
sips -z 256 256 assets/icon-256.png --out AppIcon.iconset/icon_128x128@2x.png
sips -z 256 256 assets/icon-256.png --out AppIcon.iconset/icon_256x256.png
iconutil -c icns AppIcon.iconset -o Resources/AppIcon.icns
rm -rf AppIcon.iconset

flags=(-c release --triple "$target")
swift build "${flags[@]}"
products="$(swift build "${flags[@]}" --show-bin-path)"

app="build/Snowdrift.app"
rm -rf "$app" build/Snowdrift.dmg
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$products/Snowdrift" "$app/Contents/MacOS/"
cp Resources/Info.plist "$app/Contents/"
cp Resources/AppIcon.icns "$app/Contents/Resources/"

codesign --force --deep --sign - "$app"
hdiutil create -volname "Snowdrift" -srcfolder "$app" -ov -format UDZO build/Snowdrift.dmg
echo "Built build/Snowdrift.dmg"
