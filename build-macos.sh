#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

target=arm64-apple-macosx14.0
if [ "$(uname -m)" = "x86_64" ]; then
    target=x86_64-apple-macosx14.0
fi

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
