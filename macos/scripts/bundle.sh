#!/bin/bash
# Builds build/AnyDisplay.app (menu bar app, ad-hoc signed) from the Swift package.
# Screen Recording and Accessibility permissions attach to the app's signature; an ad-hoc
# signature changes on every build, so macOS may ask again after rebuilding.
set -euo pipefail
cd "$(dirname "$0")/.."

swift build -c release --arch arm64 --arch x86_64
BIN="$(swift build -c release --arch arm64 --arch x86_64 --show-bin-path)/anydisplay"
APP=build/AnyDisplay.app
VERSION="${ANYDISPLAY_VERSION:-0.1.0}"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/anydisplay"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.anydisplay.host</string>
    <key>CFBundleName</key><string>AnyDisplay</string>
    <key>CFBundleDisplayName</key><string>AnyDisplay</string>
    <key>CFBundleExecutable</key><string>anydisplay</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHumanReadableCopyright</key><string>MIT License</string>
    <key>NSLocalNetworkUsageDescription</key><string>AnyDisplay finds and connects to your Android devices on the local network.</string>
    <key>NSBonjourServices</key><array><string>_anydisplay._tcp</string></array>
</dict>
</plist>
EOF
codesign --force --sign - --timestamp=none "$APP"
echo "built $APP"
