#!/bin/bash
set -e

SCHEME="${1:-Koe}"

echo "🔍 Locating build directory for scheme: $SCHEME"
BUILD_DIR=$(xcodebuild -project KoeApp/Koe.xcodeproj -scheme "$SCHEME" -configuration Release -showBuildSettings 2>/dev/null | grep ' BUILD_DIR' | head -1 | awk '{print $3}')
APP_PATH="$BUILD_DIR/Release/Koe.app"

if [ ! -d "$APP_PATH" ]; then
    echo "❌ App not found: $APP_PATH"
    echo "   Run 'make build' first."
    exit 1
fi

VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP_PATH/Contents/Info.plist" 2>/dev/null || echo "0.0.0")
DMG_OUT="Koe-${VERSION}.dmg"

echo "📦 Packaging Koe v$VERSION..."

VOL_NAME="Koe"
TEMP_DMG=$(mktemp -u).dmg
APP_SIZE=$(du -sm "$APP_PATH" | cut -f1)
DMG_SIZE=$((APP_SIZE + 30))

# Create read-write DMG
hdiutil create -volname "$VOL_NAME" -srcfolder "$APP_PATH" -fs HFS+ -format UDRW -size "${DMG_SIZE}m" "$TEMP_DMG"

# Mount and add Applications shortcut
MOUNT=$(hdiutil attach "$TEMP_DMG" -nobrowse | grep 'Apple_HFS' | awk '{print $3}') || MOUNT="/Volumes/$VOL_NAME"
ln -sf /Applications "$MOUNT/Applications"

# Style the DMG window with AppleScript
AS_SCRIPT=$(mktemp)
cat > "$AS_SCRIPT" <<'APPLESCRIPT'
tell application "Finder"
    try
        tell disk "Koe"
            open
            set current view of container window to icon view
            set toolbar visible of container window to false
            set statusbar visible of container window to false
            set bounds of container window to {100, 100, 500, 400}
            set theViewOptions to icon view options of container window
            set arrangement of theViewOptions to not arranged
            set icon size of theViewOptions to 80
            set position of item "Koe.app" of container window to {120, 150}
            set position of item "Applications" of container window to {280, 150}
            close
        end tell
    end try
end tell
APPLESCRIPT
osascript "$AS_SCRIPT" > /dev/null 2>&1 || true
rm -f "$AS_SCRIPT"

# Detach, compress, and clean up
hdiutil detach "$MOUNT" -force
rm -f "$DMG_OUT"
hdiutil convert "$TEMP_DMG" -format UDZO -o "$DMG_OUT"
rm -f "$TEMP_DMG"

echo "✅ DMG ready: $DMG_OUT ($(du -h "$DMG_OUT" | cut -f1))"
