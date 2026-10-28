#!/bin/sh
# Builds build/macos/AirDraw.app: Rust core, Godot export, and the Quick Look thumbnail
# extension (PlugIns/AirDrawThumbnail.appex). Signs with the first "Apple Development"
# identity (macOS ignores ad-hoc signed app extensions); SIGN_IDENTITY overrides. Release
# builds will sign with a Developer ID and notarize.
set -eu
IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')}"
[ -n "$IDENTITY" ] || { echo "No Apple Development signing identity found; set SIGN_IDENTITY" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/macos/AirDraw.app"
SRC="$ROOT/platform/apple/AirDrawThumbnail"
WORK="$ROOT/build/macos/thumbnail"

(cd "$ROOT/rust" && cargo build --release -p airdraw-godot)
mkdir -p "$ROOT/build/macos"
"$ROOT/engine/godot/bin/godot.macos.editor.arm64" --headless --path "$ROOT/app" --export-release "macOS" "$APP" 2>&1 \
  | grep -E "ERROR" || true
[ -d "$APP" ] || { echo "Godot export failed" >&2; exit 1; }

# App icon (scripts/set_app_icon.sh): compile the Icon Composer document so macOS 26 shows its
# dark, tinted and glass looks (Assets.car), with an .icns for older macOS.
ICON="$ROOT/platform/apple/AppIcon.icon"
if [ -d "$ICON" ]; then
  ICONS="$ROOT/build/macos/icon" && rm -rf "$ICONS" && mkdir -p "$ICONS"
  xcrun actool "$ICON" --compile "$ICONS" --app-icon AppIcon --platform macosx --target-device mac \
    --minimum-deployment-target 12.0 --output-partial-info-plist "$ICONS/partial.plist" >/dev/null
  cp "$ICONS/Assets.car" "$ICONS/AppIcon.icns" "$APP/Contents/Resources/"
  rm -f "$APP/Contents/Resources/icon.icns"  # Godot's, from icon.png
  /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" "$APP/Contents/Info.plist" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$APP/Contents/Info.plist"
fi

# Thumbnail extension. Extensions start in NSExtensionMain, not main().
rm -rf "$WORK" && mkdir -p "$WORK"
APPEX="$WORK/AirDrawThumbnail.appex"
mkdir -p "$APPEX/Contents/MacOS"
xcrun swiftc -target arm64-apple-macos12.0 -sdk "$(xcrun --show-sdk-path --sdk macosx)" \
  -O -module-name AirDrawThumbnail -application-extension -parse-as-library \
  -Xlinker -e -Xlinker _NSExtensionMain \
  "$SRC/ThumbnailProvider.swift" -o "$APPEX/Contents/MacOS/AirDrawThumbnail"
cp "$SRC/Info-macOS.plist" "$APPEX/Contents/Info.plist"
codesign --force --timestamp=none --sign "$IDENTITY" --entitlements "$SRC/macOS.entitlements" "$APPEX"

rm -rf "$APP/Contents/PlugIns/AirDrawThumbnail.appex"
mkdir -p "$APP/Contents/PlugIns"
cp -R "$APPEX" "$APP/Contents/PlugIns/"
# Sign inside-out with one identity: libraries, the extension (above), then the app,
# keeping the entitlements Godot's export set.
for lib in "$APP"/Contents/Frameworks/*.dylib; do
  codesign --force --timestamp=none --sign "$IDENTITY" "$lib"
done
codesign --force --timestamp=none --sign "$IDENTITY" --preserve-metadata=entitlements,flags,runtime "$APP"

# Make Launch Services and Quick Look pick up the new version.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP"
echo "Built $APP"
